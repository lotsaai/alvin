use std::cell::RefCell;
use std::error::Error as StdError;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use alvin_stt::{PreparationProgress, Transcriber};
use gpui_kit::component::speech::{
  AudioFormat, AudioInput, AudioSink, Microphone, RecognitionSession, SpeechError, SpeechEvent, SpeechRecognizer, SpeechSink, SpeechState,
};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task};

use crate::Error;

type Model = Arc<Mutex<Transcriber>>;
fn format_error(error: &dyn StdError) -> String {
  let mut message = error.to_string();
  let mut source = error.source();
  while let Some(error) = source {
    let detail = error.to_string();
    if !message.contains(&detail) {
      message.push_str(": ");
      message.push_str(&detail);
    }
    source = error.source();
  }
  message
}

struct SharedSpeech(Entity<LocalSpeech>);
impl Global for SharedSpeech {}

pub(crate) fn init(cache_dir: PathBuf, cx: &mut App) {
  let speech = cx.new(|_| LocalSpeech {
    cache_dir,
    state: ModelState::Unloaded,
    task: None,
    download_started: None,
    workers: Vec::new(),
  });
  cx.set_global(SharedSpeech(speech));
}

/// Cancel model preparation and wait for local speech work to finish.
///
/// Cancel retained workspace dictation before calling this during application shutdown.
pub fn shutdown(cx: &mut App) -> Task<()> {
  let shared = cx.global::<SharedSpeech>().0.clone();
  let workers = shared.update(cx, |shared, _| {
    shared.task.take();
    std::mem::take(&mut shared.workers)
  });
  cx.background_spawn(async move {
    for worker in workers {
      worker.await;
    }
  })
}

enum ModelState {
  Unloaded,
  Preparing(Option<PreparationProgress>),
  Ready(Model),
  Failed(Error),
}

struct LocalSpeech {
  cache_dir: PathBuf,
  state: ModelState,
  task: Option<Task<()>>,
  download_started: Option<(Instant, u64)>,
  workers: Vec<Task<()>>,
}

enum Preparation {
  Progress(PreparationProgress),
  Done(alvin_stt::Result<Transcriber>),
}

impl LocalSpeech {
  fn prepare(&mut self, cx: &mut Context<Self>) {
    if !matches!(self.state, ModelState::Unloaded | ModelState::Failed(_)) {
      return;
    }
    self.state = ModelState::Preparing(None);
    self.download_started = None;
    cx.notify();
    let cache_dir = self.cache_dir.clone();
    let (tx, rx) = async_channel::bounded(1);
    self.workers.retain(|worker| !worker.is_ready());
    self.workers.push(cx.background_spawn(async move {
      let result = Transcriber::load(&cache_dir, |progress| {
        if tx.force_send(Preparation::Progress(progress)).is_ok() {
          ControlFlow::Continue(())
        } else {
          ControlFlow::Break(())
        }
      });
      if tx.force_send(Preparation::Done(result)).is_err() {
        tracing::debug!("speech preparation consumer closed");
      }
    }));
    self.task = Some(cx.spawn(async move |this, cx| {
      while let Ok(message) = rx.recv().await {
        if this
          .update(cx, |this, cx| {
            this.state = match message {
              Preparation::Progress(progress) => {
                if let PreparationProgress::Downloading { completed_bytes, .. } = progress {
                  this.download_started.get_or_insert((Instant::now(), completed_bytes));
                }
                ModelState::Preparing(Some(progress))
              }
              Preparation::Done(Ok(model)) => ModelState::Ready(Arc::new(Mutex::new(model))),
              Preparation::Done(Err(source)) => ModelState::Failed(Error::Speech { source }),
            };
            cx.notify();
          })
          .is_err()
        {
          break;
        }
      }
    }));
  }
}

#[derive(Debug, Clone)]
pub(crate) enum Phase {
  Idle,
  Preparing(Option<PreparationProgress>),
  Capturing,
  Transcribing,
  Failed(String),
}

#[derive(Default)]
struct Capture {
  audio: Option<async_channel::Sender<Vec<i16>>>,
  generation: Rc<()>,
  sink: Option<SpeechSink>,
}

struct Recognizer(Rc<RefCell<Capture>>);

impl SpeechRecognizer for Recognizer {
  fn start(&self, sink: SpeechSink, cx: &mut App) -> Result<Box<dyn RecognitionSession>, SpeechError> {
    *self.0.borrow_mut() = Capture {
      audio: None,
      generation: Rc::new(()),
      sink: Some(sink.clone()),
    };
    sink.ready(cx);
    Ok(Box::new(Recording(self.0.clone())))
  }
}

struct Recording(Rc<RefCell<Capture>>);
impl RecognitionSession for Recording {
  fn push_audio(&mut self, samples: &[i16], cx: &mut App) {
    let capture = self.0.borrow();
    if let Some(audio) = &capture.audio
      && let Err(error) = audio.try_send(samples.to_vec())
      && let Some(sink) = &capture.sink
    {
      sink.error(SpeechError::recognizer(error), cx);
    }
  }

  fn finish(&mut self, cx: &mut App) {
    if let Some(sink) = self.0.borrow().sink.as_ref() {
      sink.error(SpeechError::Unsupported, cx);
    }
  }
}

struct RetainedInput {
  input: Rc<RefCell<Rc<dyn AudioInput>>>,
  capture: Rc<RefCell<Option<Subscription>>>,
}

impl AudioInput for RetainedInput {
  fn start(&self, format: AudioFormat, sink: AudioSink, cx: &mut App) -> Result<Subscription, SpeechError> {
    *self.capture.borrow_mut() = Some(self.input.borrow().start(format, sink, cx)?);
    let capture = self.capture.clone();
    Ok(Subscription::new(move || drop(capture.borrow_mut().take())))
  }
}

pub(crate) struct Dictation {
  phase: Phase,
  shared: Entity<LocalSpeech>,
  speech: Entity<SpeechState>,
  capture: Rc<RefCell<Capture>>,
  microphone: Rc<RefCell<Option<Subscription>>>,
  #[cfg(test)]
  input: Rc<RefCell<Rc<dyn AudioInput>>>,
  #[cfg(test)]
  inference_gate: Option<test_support::InferenceGate>,
  job: Option<Task<()>>,
  _subscriptions: Vec<Subscription>,
}

impl Drop for Dictation {
  fn drop(&mut self) {
    drop(self.microphone.borrow_mut().take());
    self.capture.borrow_mut().audio.take();
    self.job.take();
  }
}

impl EventEmitter<SpeechEvent> for Dictation {}

pub(super) struct DictatedText {
  pub(super) generation: Rc<()>,
  pub(super) text: SharedString,
}

impl EventEmitter<DictatedText> for Dictation {}

impl Dictation {
  pub(super) fn accepts_generation(&self, generation: &Rc<()>) -> bool {
    Rc::ptr_eq(generation, &self.capture.borrow().generation)
  }

  pub(crate) fn new(cx: &mut Context<Self>) -> Self {
    Self::with_input(Rc::new(Microphone::default()), cx)
  }

  pub(super) fn with_input(input: Rc<dyn AudioInput>, cx: &mut Context<Self>) -> Self {
    let shared = cx.global::<SharedSpeech>().0.clone();
    let capture = Rc::new(RefCell::new(Capture::default()));
    let microphone = Rc::new(RefCell::new(None));
    let input = Rc::new(RefCell::new(input));
    let speech = cx.new(|cx| {
      SpeechState::new(cx)
        .recognizer(Recognizer(capture.clone()))
        .input(RetainedInput {
          input: input.clone(),
          capture: microphone.clone(),
        })
        .system_fallback(false)
    });
    let subscriptions = vec![
      cx.observe(&shared, |this, _, cx| this.model_changed(cx)),
      cx.subscribe(&speech, |this, speech, event: &SpeechEvent, cx| {
        if matches!(event, SpeechEvent::Final(_) | SpeechEvent::Error(_)) && speech.read(cx).status().is_active() {
          return;
        }
        match event {
          SpeechEvent::Final(_) if matches!(this.phase, Phase::Transcribing) => this.phase = Phase::Idle,
          SpeechEvent::Error(error) if matches!(this.phase, Phase::Capturing | Phase::Transcribing) => {
            this.phase = Phase::Failed(format_error(&Error::SpeechCapture { source: error.clone() }));
            drop(this.microphone.borrow_mut().take());
            this.capture.borrow_mut().audio.take();
            this.job.take();
          }
          SpeechEvent::Started | SpeechEvent::Partial(_) if matches!(this.phase, Phase::Capturing | Phase::Transcribing) => {}
          SpeechEvent::Final(_) | SpeechEvent::Cancelled | SpeechEvent::Error(_) | SpeechEvent::Started | SpeechEvent::Partial(_) => return,
        }
        cx.emit(event.clone());
        cx.notify();
      }),
    ];
    Self {
      phase: Phase::Idle,
      shared,
      speech,
      capture,
      microphone,
      job: None,
      _subscriptions: subscriptions,
      #[cfg(test)]
      input,
      #[cfg(test)]
      inference_gate: None,
    }
  }

  pub(crate) fn phase(&self) -> &Phase {
    &self.phase
  }
  pub(crate) fn remaining(&self, cx: &App) -> Option<Duration> {
    let shared = self.shared.read(cx);
    let ModelState::Preparing(Some(PreparationProgress::Downloading {
      completed_bytes,
      total_bytes,
    })) = shared.state
    else {
      return None;
    };
    let (started, initial_bytes) = shared.download_started?;
    Self::estimate_remaining(started.elapsed(), initial_bytes, completed_bytes, total_bytes)
  }
  fn estimate_remaining(elapsed: Duration, initial: u64, completed: u64, total: u64) -> Option<Duration> {
    let downloaded = completed.saturating_sub(initial);
    let remaining = total.saturating_sub(completed);
    let nanos = elapsed
      .as_nanos()
      .saturating_mul(u128::from(remaining))
      .checked_div(u128::from(downloaded))?;
    let seconds = nanos.div_ceil(1_000_000_000).min(u128::from(u64::MAX)) as u64;
    Some(Duration::from_secs(seconds))
  }
  pub(crate) fn speech(&self) -> &Entity<SpeechState> {
    &self.speech
  }
  #[cfg(test)]
  pub(crate) fn set_input(&mut self, input: Rc<dyn AudioInput>) {
    *self.input.borrow_mut() = input;
  }
  #[cfg(test)]
  pub(crate) fn gate_inference(&mut self, gate: test_support::InferenceGate) {
    self.inference_gate = Some(gate);
  }

  pub(crate) fn toggle(&mut self, cx: &mut Context<Self>) {
    match self.phase {
      Phase::Idle | Phase::Failed(_) => {
        self.phase = Phase::Preparing(None);
        self.shared.update(cx, |shared, cx| shared.prepare(cx));
        self.model_changed(cx);
      }
      Phase::Preparing(_) => self.cancel(cx),
      Phase::Capturing => self.stop(cx),
      Phase::Transcribing => {}
    }
  }

  fn model_changed(&mut self, cx: &mut Context<Self>) {
    if !matches!(self.phase, Phase::Preparing(_)) {
      return;
    }
    match &self.shared.read(cx).state {
      ModelState::Unloaded => {}
      ModelState::Preparing(progress) => self.phase = Phase::Preparing(*progress),
      ModelState::Failed(error) => self.phase = Phase::Failed(format_error(error)),
      ModelState::Ready(model) => {
        let model = model.clone();
        self.phase = Phase::Capturing;
        self.speech.update(cx, |speech, cx| speech.start(cx));
        self.start_stream(model, cx);
      }
    }
    cx.notify();
  }

  fn stop(&mut self, cx: &mut Context<Self>) {
    drop(self.microphone.borrow_mut().take());
    self.phase = Phase::Transcribing;
    cx.notify();
    let this = cx.weak_entity();
    let generation = self.capture.borrow().generation.clone();
    cx.defer(move |cx| {
      if this
        .update(cx, |this, _| {
          if matches!(this.phase, Phase::Transcribing) && Rc::ptr_eq(&generation, &this.capture.borrow().generation) {
            this.capture.borrow_mut().audio.take();
          }
        })
        .is_err()
      {
        tracing::debug!("dictation view closed before capture finished");
      }
    });
  }

  fn start_stream(&mut self, model: Model, cx: &mut Context<Self>) {
    let (audio, input) = async_channel::unbounded();
    let (sender, receiver) = async_channel::unbounded();
    let (generation, sink) = {
      let mut capture = self.capture.borrow_mut();
      capture.audio = Some(audio);
      (capture.generation.clone(), capture.sink.clone())
    };
    let Some(sink) = sink else {
      return;
    };
    #[cfg(test)]
    let gate = self.inference_gate.take();
    let worker = cx.background_spawn(async move {
      let mut stream = model.lock().unwrap_or_else(PoisonError::into_inner).stream();
      #[cfg(test)]
      if let Some(gate) = &gate {
        gate.started.send(()).await.expect("inference start receiver");
        gate.release.recv().await.expect("inference release sender");
      }
      let mut active = true;
      while let Ok(samples) = input.recv().await {
        let result = stream.push(&samples);
        let failed = result.is_err();
        if sender.send((result, false)).await.is_err() || failed {
          active = false;
          break;
        }
      }
      if active {
        let result = stream.finish();
        if sender.send((result, true)).await.is_err() {
          tracing::debug!("dictation result consumer closed");
        }
      }
      #[cfg(test)]
      if let Some(gate) = gate {
        gate.finished.send(()).await.expect("inference finish receiver");
      }
    });
    self.shared.update(cx, |shared, _| {
      shared.workers.retain(|worker| !worker.is_ready());
      shared.workers.push(worker);
    });
    self.job = Some(cx.spawn(async move |this, cx| {
      while let Ok((result, finished)) = receiver.recv().await {
        if this
          .update(cx, |this, cx| {
            if !matches!(this.phase, Phase::Capturing | Phase::Transcribing) || !Rc::ptr_eq(&generation, &this.capture.borrow().generation)
            {
              return;
            }
            match result {
              Ok(text) => {
                if !text.is_empty() {
                  let text: SharedString = text.into();
                  cx.emit(DictatedText {
                    generation: generation.clone(),
                    text: text.clone(),
                  });
                  sink.phrase(text, cx);
                }
                if finished {
                  sink.finish(cx);
                }
              }
              Err(source) => sink.error(SpeechError::recognizer(Error::Speech { source }), cx),
            }
          })
          .is_err()
        {
          break;
        }
      }
    }));
  }

  pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
    drop(self.microphone.borrow_mut().take());
    self.job.take();
    self.phase = Phase::Idle;
    self.speech.update(cx, |speech, cx| speech.cancel(cx));
    *self.capture.borrow_mut() = Capture::default();
    cx.notify();
  }
}

#[cfg(test)]
pub(crate) mod test_support {
  use std::cell::Cell;
  use std::rc::Rc;

  use gpui_kit::component::speech::{AudioFormat, AudioInput, AudioSink, SpeechError};
  use gpui_kit::{App, AppContext as _, Context, Entity, Subscription};

  use super::{LocalSpeech, ModelState, SharedSpeech};

  pub(crate) struct InferenceGate {
    pub(crate) started: async_channel::Sender<()>,
    pub(crate) release: async_channel::Receiver<()>,
    pub(crate) finished: async_channel::Sender<()>,
  }

  pub(crate) struct PreparationProbe {
    _subscription: Subscription,
  }

  pub(crate) fn watch_preparation(cx: &mut App) -> (Entity<PreparationProbe>, async_channel::Receiver<Result<(), String>>) {
    let shared = cx.global::<SharedSpeech>().0.clone();
    let (sender, receiver) = async_channel::bounded(1);
    let probe = cx.new(|cx: &mut Context<PreparationProbe>| {
      let report = move |shared: &LocalSpeech| {
        let result = match &shared.state {
          ModelState::Ready(_) => Ok(()),
          ModelState::Failed(error) => Err(super::format_error(error)),
          ModelState::Unloaded | ModelState::Preparing(_) => return,
        };
        if let Err(error) = sender.try_send(result) {
          tracing::debug!(%error, "preparation observer closed");
        }
      };
      report(shared.read(cx));
      let subscription = cx.observe(&shared, move |_, shared, cx| report(shared.read(cx)));
      PreparationProbe {
        _subscription: subscription,
      }
    });
    (probe, receiver)
  }

  struct FileInput {
    samples: Vec<i16>,
    delivered: async_channel::Sender<()>,
  }

  impl AudioInput for FileInput {
    fn start(&self, format: AudioFormat, sink: AudioSink, cx: &mut App) -> Result<Subscription, SpeechError> {
      assert_eq!(format, AudioFormat::default());
      let samples = self.samples.clone();
      let delivered = self.delivered.clone();
      let active = Rc::new(Cell::new(true));
      let capturing = active.clone();
      cx.defer(move |cx| {
        if capturing.get() {
          sink.push(samples, cx);
          cx.defer(move |_| {
            delivered.try_send(()).expect("file delivery receiver");
          });
        }
      });
      Ok(Subscription::new(move || active.set(false)))
    }
  }

  fn file_input_with_limit(limit: usize) -> (Rc<dyn AudioInput>, async_channel::Receiver<()>) {
    let mut reader = hound::WavReader::open(concat!(env!("CARGO_MANIFEST_DIR"), "/../alvin-stt/fixtures/jfk.wav")).unwrap();
    let spec = reader.spec();
    assert_eq!((spec.sample_rate, spec.channels, spec.bits_per_sample), (16_000, 1, 16));
    let samples = reader.samples().take(limit).collect::<std::result::Result<Vec<i16>, _>>().unwrap();
    let (delivered, receiver) = async_channel::unbounded();
    (Rc::new(FileInput { samples, delivered }), receiver)
  }

  pub(crate) fn file_input() -> (Rc<dyn AudioInput>, async_channel::Receiver<()>) {
    file_input_with_limit(usize::MAX)
  }

  pub(crate) fn short_file_input() -> (Rc<dyn AudioInput>, async_channel::Receiver<()>) {
    file_input_with_limit(159)
  }

  pub(crate) fn empty_file_input() -> (Rc<dyn AudioInput>, async_channel::Receiver<()>) {
    file_input_with_limit(0)
  }
}

#[cfg(test)]
mod tests {
  use std::error::Error as _;
  use std::path::PathBuf;
  use std::time::Duration;

  use gpui_kit::{AppContext as _, TestAppContext};

  use super::{Dictation, ModelState, Phase, format_error, init, test_support};
  use crate::Error;

  #[test]
  fn download_estimate_excludes_cached_bytes() {
    assert_eq!(Dictation::estimate_remaining(Duration::from_secs(10), 50, 50, 150), None);
    assert_eq!(
      Dictation::estimate_remaining(Duration::from_secs(10), 50, 75, 150),
      Some(Duration::from_secs(30))
    );
    assert_eq!(
      Dictation::estimate_remaining(Duration::from_millis(1), 0, 1, 2),
      Some(Duration::from_secs(1))
    );
  }

  #[test]
  fn preparation_failure_keeps_typed_source_and_displays_io_cause() {
    let error = Error::Speech {
      source: alvin_stt::Error::CacheIo {
        path: PathBuf::from("/speech/cache"),
        source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied"),
      },
    };
    assert!(error.source().unwrap().downcast_ref::<alvin_stt::Error>().is_some());
    let message = format_error(&error);
    assert!(message.contains("/speech/cache"));
    assert!(message.contains("permission denied"));
    assert!(matches!(ModelState::Failed(error), ModelState::Failed(Error::Speech { .. })));
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn retry_rechecks_real_cache_after_preparation_failure(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    tokio::task::coop::unconstrained(async {
      executor.block_test(async {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("speech");
        std::fs::write(&cache, b"not a directory").unwrap();
        let (input, _delivered) = test_support::empty_file_input();
        let (dictation, _probe, failed) = cx.update(|cx| {
          init(cache.clone(), cx);
          let dictation = cx.new(|cx| Dictation::with_input(input, cx));
          let (probe, failed) = test_support::watch_preparation(cx);
          dictation.update(cx, |dictation, cx| dictation.toggle(cx));
          (dictation, probe, failed)
        });
        let first = failed.recv().await.unwrap().unwrap_err();
        assert!(first.contains("speech"));

        std::fs::remove_file(&cache).unwrap();
        std::fs::create_dir(&cache).unwrap();
        let model_dir = cache.join("nemotron-3.5-asr-streaming-0.6b-onnx");
        std::fs::create_dir(&model_dir).unwrap();
        std::fs::write(model_dir.join("664cc742197053fa11afd12f5100749fb8000181"), b"not a directory").unwrap();
        let (_retry_probe, retried) = cx.update(|cx| {
          dictation.update(cx, |dictation, cx| dictation.toggle(cx));
          test_support::watch_preparation(cx)
        });
        let second = retried.recv().await.unwrap().unwrap_err();
        assert!(second.contains("nemotron-3.5-asr-streaming-0.6b-onnx"));
        assert_ne!(first, second);
      });
    })
    .await;
  }

  #[gpui_kit::test]
  fn cancellation_does_not_end_replacement_capture(cx: &mut TestAppContext) {
    let (input, _delivered) = test_support::file_input();
    let dictation = cx.update(|cx| {
      init(PathBuf::new(), cx);
      let dictation = cx.new(|cx| Dictation::with_input(input, cx));
      dictation.update(cx, |dictation, cx| {
        dictation.phase = Phase::Capturing;
        dictation.speech.update(cx, |speech, cx| speech.start(cx));
        dictation.cancel(cx);
        dictation.phase = Phase::Capturing;
        dictation.speech.update(cx, |speech, cx| speech.start(cx));
      });
      dictation
    });
    cx.run_until_parked();
    cx.update(|cx| {
      assert!(matches!(dictation.read(cx).phase(), Phase::Capturing));
      assert!(dictation.read(cx).speech().read(cx).status().is_capturing());
      dictation.update(cx, |dictation, cx| dictation.cancel(cx));
    });
  }

  #[gpui_kit::test]
  fn dropping_dictation_stops_capture_when_speech_state_is_retained(cx: &mut TestAppContext) {
    let (input, delivered) = test_support::file_input();
    let (sender, receiver) = async_channel::unbounded::<Vec<i16>>();
    let speech = cx.update(|cx| {
      init(PathBuf::new(), cx);
      let dictation = cx.new(|cx| Dictation::with_input(input, cx));
      let speech = dictation.read(cx).speech().clone();
      dictation.update(cx, |dictation, cx| {
        dictation.phase = Phase::Capturing;
        dictation.speech.update(cx, |speech, cx| speech.start(cx));
        dictation.capture.borrow_mut().audio = Some(sender);
      });
      drop(dictation);
      speech
    });
    cx.run_until_parked();
    assert!(delivered.try_recv().is_err());
    assert!(receiver.is_closed());
    cx.update(|cx| speech.update(cx, |speech, cx| speech.cancel(cx)));
  }
}
