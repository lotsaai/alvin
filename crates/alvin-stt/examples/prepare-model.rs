use std::ops::ControlFlow;
use std::path::PathBuf;

use alvin_stt::{PreparationProgress, prepare};

fn main() -> alvin_stt::Result<()> {
  let cache = PathBuf::from(std::env::args_os().nth(1).expect("usage: prepare-model CACHE_DIRECTORY"));
  let mut downloading = false;
  prepare(&cache, &mut |progress| {
    if let PreparationProgress::Downloading {
      completed_bytes,
      total_bytes,
    } = progress
      && !downloading
    {
      eprintln!(
        "Preparing speech model in {} ({completed_bytes}/{total_bytes} bytes cached)",
        cache.display()
      );
      downloading = true;
    }
    ControlFlow::Continue(())
  })?;
  eprintln!("Speech model ready in {} (downloaded: {downloading})", cache.display());
  Ok(())
}
