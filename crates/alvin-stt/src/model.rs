use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

const REVISION: &str = "664cc742197053fa11afd12f5100749fb8000181";
const MODEL: &str = "nemotron-3.5-asr-streaming-0.6b-onnx";
const ASSETS: [(&str, u64); 4] = [
  ("encoder.onnx", 4_161_952),
  ("encoder.onnx.data", 2_495_361_024),
  ("decoder_joint.onnx", 97_653_235),
  ("tokenizer.model", 406_554),
];
const TOTAL: u64 = 2_597_582_765;

/// Progress preparing the local speech model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationProgress {
  /// Downloaded and total bytes across the complete model manifest.
  Downloading { completed_bytes: u64, total_bytes: u64 },
  /// Opening the downloaded model.
  Loading,
}

/// Download missing model assets into the revision-keyed cache without loading inference.
///
/// # Errors
/// Returns cache IO, download, asset-length, or cancellation errors.
pub fn prepare(root: &Path, progress: &mut impl FnMut(PreparationProgress) -> ControlFlow<()>) -> Result<PathBuf> {
  let dir = root.join(MODEL).join(REVISION);
  fs::create_dir_all(&dir).map_err(|source| Error::CacheIo { path: dir.clone(), source })?;
  let mut missing = Vec::new();
  let mut completed = 0_u64;
  for (name, expected) in ASSETS {
    let path = dir.join(name);
    match fs::metadata(&path) {
      Ok(meta) if meta.is_file() && meta.len() == expected => completed += expected,
      Ok(_) => missing.push((name, expected)),
      Err(source) if source.kind() == ErrorKind::NotFound => missing.push((name, expected)),
      Err(source) => return Err(Error::CacheIo { path, source }),
    }
  }
  if !missing.is_empty() {
    report(
      progress,
      PreparationProgress::Downloading {
        completed_bytes: completed,
        total_bytes: TOTAL,
      },
    )?;
  }
  for (name, expected) in missing {
    let path = dir.join(name);
    let part = dir.join(format!("{name}.part"));
    let url = format!("https://huggingface.co/altunenes/parakeet-rs/resolve/{REVISION}/{MODEL}/{name}");
    let mut response = ureq::get(&url).call().map_err(|source| Error::Download {
      path: path.clone(),
      source,
    })?;
    let mut output = File::create(&part).map_err(|source| Error::DownloadIo {
      path: part.clone(),
      source,
    })?;
    let mut buffer = [0_u8; 65536];
    let mut actual = 0_u64;
    let mut reader = response.body_mut().as_reader();
    loop {
      let n = reader.read(&mut buffer).map_err(|source| Error::DownloadIo {
        path: part.clone(),
        source,
      })?;
      if n == 0 {
        break;
      }
      let chunk = buffer.get(..n).ok_or_else(|| Error::DownloadIo {
        path: part.clone(),
        source: std::io::Error::new(ErrorKind::InvalidData, "reader returned an invalid length"),
      })?;
      output.write_all(chunk).map_err(|source| Error::DownloadIo {
        path: part.clone(),
        source,
      })?;
      actual += n as u64;
      report(
        progress,
        PreparationProgress::Downloading {
          completed_bytes: completed.saturating_add(actual),
          total_bytes: TOTAL,
        },
      )?;
    }
    if actual != expected {
      return Err(Error::AssetLength { path, expected, actual });
    }
    drop(output);
    fs::rename(&part, &path).map_err(|source| Error::CacheIo {
      path: path.clone(),
      source,
    })?;
    completed += expected;
  }
  report(progress, PreparationProgress::Loading)?;
  Ok(dir)
}

fn report(progress: &mut impl FnMut(PreparationProgress) -> ControlFlow<()>, value: PreparationProgress) -> Result<()> {
  match progress(value) {
    ControlFlow::Continue(()) => Ok(()),
    ControlFlow::Break(()) => Err(Error::Cancelled),
  }
}

#[cfg(test)]
mod tests {
  use std::fs::{self, File};
  use std::ops::ControlFlow;

  use tempfile::TempDir;
  use test_context::{AsyncTestContext, test_context};

  use super::{ASSETS, MODEL, PreparationProgress, REVISION, TOTAL, prepare};
  use crate::Error;

  struct Fixture(TempDir);

  impl AsyncTestContext for Fixture {
    async fn setup() -> Self {
      Self(TempDir::new().unwrap())
    }
  }

  #[test_context(Fixture)]
  #[tokio::test]
  async fn incomplete_cache_reports_completed_bytes_and_cancels(fixture: &mut Fixture) {
    let dir = fixture.0.path().join(MODEL).join(REVISION);
    fs::create_dir_all(&dir).unwrap();
    File::create(dir.join("tokenizer.model")).unwrap().set_len(406_554).unwrap();
    File::create(dir.join("encoder.onnx")).unwrap().set_len(5).unwrap();
    File::create(dir.join("decoder_joint.onnx.part"))
      .unwrap()
      .set_len(97_653_235)
      .unwrap();
    let mut seen = Vec::new();
    let result = prepare(fixture.0.path(), &mut |progress| {
      seen.push(progress);
      ControlFlow::Break(())
    });
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(
      seen,
      vec![PreparationProgress::Downloading {
        completed_bytes: 406_554,
        total_bytes: TOTAL
      }]
    );
  }

  #[test_context(Fixture)]
  #[tokio::test]
  async fn complete_cache_needs_no_download(fixture: &mut Fixture) {
    let dir = fixture.0.path().join(MODEL).join(REVISION);
    fs::create_dir_all(&dir).unwrap();
    for (name, length) in ASSETS {
      File::create(dir.join(name)).unwrap().set_len(length).unwrap();
    }
    let mut seen = Vec::new();
    assert_eq!(
      prepare(fixture.0.path(), &mut |progress| {
        seen.push(progress);
        ControlFlow::Continue(())
      })
      .unwrap(),
      dir
    );
    assert_eq!(seen, vec![PreparationProgress::Loading]);
  }

  #[test_context(Fixture)]
  #[tokio::test]
  async fn cache_io_failure_is_not_treated_as_missing(fixture: &mut Fixture) {
    let file = fixture.0.path().join("not-a-directory");
    File::create(&file).unwrap();
    assert!(matches!(
      prepare(&file, &mut |_| ControlFlow::Continue(())),
      Err(Error::CacheIo { .. })
    ));
  }
}
