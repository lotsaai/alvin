use std::fmt;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use parakeet_rs::ort::environment::Environment;
use parakeet_rs::{ExecutionConfig, ExecutionProvider, Nemotron, NemotronHandle};

use crate::model::{self, PreparationProgress};
use crate::{Error, Result};

static ENVIRONMENT: LazyLock<Mutex<Option<Environment>>> = LazyLock::new(|| Mutex::new(None));

/// Loaded local model. Preparation and inference block the calling thread.
pub struct Transcriber {
  model: NemotronHandle,
  model_dir: PathBuf,
  _environment: Environment,
}

impl fmt::Debug for Transcriber {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Transcriber")
      .field("checkpoint", &"nemotron-3.5-asr-streaming-0.6b-onnx")
      .field("model_dir", &self.model_dir)
      .finish_non_exhaustive()
  }
}

impl Transcriber {
  /// Prepare the cache and load a model. Returning `Break` cancels between IO stages.
  ///
  /// # Errors
  /// Returns preparation, runtime, and model loading errors.
  pub fn load(cache_root: &Path, mut progress: impl FnMut(PreparationProgress) -> ControlFlow<()>) -> Result<Self> {
    let model_dir = model::prepare(cache_root, &mut progress)?;
    let mut shared = ENVIRONMENT.lock().unwrap_or_else(PoisonError::into_inner);
    let environment = match shared.as_ref() {
      Some(env) => env.clone(),
      None => {
        let env = parakeet_rs::ort::init().build().map_err(|source| Error::Runtime { source })?;
        *shared = Some(env.clone());
        env
      }
    };
    drop(shared);
    let config = ExecutionConfig::new()
      .with_environment(environment.clone())
      .with_execution_provider(ExecutionProvider::Cpu);
    let model = NemotronHandle::from_pretrained(&model_dir, Some(config)).map_err(|source| Error::ModelLoad {
      path: model_dir.clone(),
      source,
    })?;
    Ok(Self {
      model,
      model_dir,
      _environment: environment,
    })
  }

  /// Start an independent stateful stream sharing this loaded model.
  pub fn stream(&self) -> Stream {
    Stream {
      model: Nemotron::from_shared(&self.model),
      pending: Vec::new(),
      received: 0,
      emitted: false,
    }
  }

  /// Transcribe 16 kHz mono signed PCM16 without mutating the input. Blocks for CPU inference.
  ///
  /// # Errors
  /// Returns an inference error if the model fails.
  pub fn transcribe(&mut self, pcm: &[i16]) -> Result<String> {
    if pcm.len() < 160 {
      return Ok(String::new());
    }
    let mut stream = self.stream();
    let mut text = stream.push(pcm)?;
    text.push_str(&stream.finish()?);
    Ok(text)
  }
}

/// Stateful native streaming recognition for 16 kHz mono signed PCM16.
pub struct Stream {
  model: Nemotron,
  pending: Vec<f32>,
  received: usize,
  emitted: bool,
}

impl fmt::Debug for Stream {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Stream").finish_non_exhaustive()
  }
}

impl Stream {
  /// Feed audio and return newly recognized text. Performs blocking CPU work.
  ///
  /// # Errors
  /// Returns an inference error if the model fails.
  pub fn push(&mut self, pcm: &[i16]) -> Result<String> {
    self.received = self.received.saturating_add(pcm.len());
    self.pending.extend(pcm.iter().map(|sample| f32::from(*sample) / 32768.0));
    let size = self.model.chunk_samples();
    let mut text = String::new();
    let mut consumed: usize = 0;
    while let Some(end) = consumed.checked_add(size) {
      let Some(chunk) = self.pending.get(consumed..end) else { break };
      text.push_str(
        &self
          .model
          .transcribe_chunk(chunk)
          .map_err(|source| Error::Transcription { source })?,
      );
      consumed = end;
    }
    self.pending.drain(..consumed);
    Ok(self.first_text(text))
  }

  /// Flush remaining audio and decoder context, returning newly recognized text.
  ///
  /// # Errors
  /// Returns an inference error if the model fails.
  pub fn finish(mut self) -> Result<String> {
    if self.received < 160 {
      return Ok(String::new());
    }
    let size = self.model.chunk_samples();
    let mut text = String::new();
    if !self.pending.is_empty() {
      self.pending.resize(size, 0.0);
      text.push_str(
        &self
          .model
          .transcribe_chunk(&self.pending)
          .map_err(|source| Error::Transcription { source })?,
      );
    }
    self.pending.clear();
    self.pending.resize(size, 0.0);
    for _ in 0..3 {
      text.push_str(
        &self
          .model
          .transcribe_chunk(&self.pending)
          .map_err(|source| Error::Transcription { source })?,
      );
    }
    Ok(self.first_text(text))
  }

  fn first_text(&mut self, mut text: String) -> String {
    if !self.emitted {
      let leading = text.len().saturating_sub(text.trim_start().len());
      text.drain(..leading);
      self.emitted = !text.is_empty();
    }
    text
  }
}

#[cfg(test)]
mod tests {
  use std::ops::ControlFlow;

  use tempfile::TempDir;
  use test_context::{AsyncTestContext, test_context};

  use crate::Transcriber;

  struct Fixture(TempDir);

  impl AsyncTestContext for Fixture {
    async fn setup() -> Self {
      Self(TempDir::new().unwrap())
    }
  }

  #[test_context(Fixture)]
  #[tokio::test]
  async fn transcribes_real_audio_twice(fixture: &mut Fixture) {
    let mut model = Transcriber::load(fixture.0.path(), |_| ControlFlow::Continue(())).unwrap();
    let mut reader = hound::WavReader::open(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/jfk.wav")).unwrap();
    let samples: Vec<i16> = reader.samples().collect::<std::result::Result<_, _>>().unwrap();
    assert_eq!(model.transcribe(&[]).unwrap(), "");
    for _ in 0..2 {
      let mut stream = model.stream();
      let mut text = String::new();
      let mut partial = false;
      for chunk in samples.chunks(8960) {
        let next = stream.push(chunk).unwrap();
        partial |= !next.is_empty();
        text.push_str(&next);
      }
      assert!(partial, "speech must be recognized before finish");
      text.push_str(&stream.finish().unwrap());
      let normalized = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
      assert!(normalized.contains("ask not what your country can do for you"), "{text}");
      assert!(normalized.contains("what you can do for your country"), "{text}");
    }
  }
}
