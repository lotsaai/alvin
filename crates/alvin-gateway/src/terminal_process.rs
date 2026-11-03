//! Operating-system ownership of a terminal's process tree.

use std::io;

#[cfg(unix)]
#[derive(Debug)]
pub(crate) struct ProcessTree {
  session: libc::pid_t,
}

#[cfg(unix)]
impl ProcessTree {
  pub(crate) fn new(child: &dyn portable_pty::Child) -> io::Result<Self> {
    let pid = child.process_id().ok_or_else(|| io::Error::other("shell has no process id"))?;
    let session = libc::pid_t::try_from(pid).map_err(io::Error::other)?;
    Ok(Self { session })
  }

  pub(crate) fn terminate(&mut self) -> io::Result<()> {
    let mut stopped = std::collections::HashSet::new();
    let mut first = None;
    loop {
      let pids = match session_members(self.session) {
        Ok(pids) => pids,
        Err(error) => {
          first = Some(error);
          break;
        }
      };
      let mut added = false;
      for pid in pids {
        if stopped.insert(pid) {
          if let Err(error) = signal(pid, libc::SIGSTOP) {
            first.get_or_insert(error);
          }
          added = true;
        }
      }
      if !added || first.is_some() {
        break;
      }
    }
    for pid in stopped {
      if let Err(error) = signal(pid, libc::SIGKILL) {
        first.get_or_insert(error);
      }
    }
    first.map_or(Ok(()), Err)
  }
}

#[cfg(unix)]
fn signal(pid: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
  // SAFETY: pid is a positive process ID obtained from the process table.
  if unsafe { libc::kill(pid, signal) } == 0 {
    return Ok(());
  }
  let error = io::Error::last_os_error();
  if error.raw_os_error() == Some(libc::ESRCH) {
    Ok(())
  } else {
    Err(error)
  }
}

#[cfg(target_os = "macos")]
fn session_members(session: libc::pid_t) -> io::Result<Vec<libc::pid_t>> {
  // SAFETY: null buffer requests the process count.
  let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
  if count < 0 {
    return Err(io::Error::last_os_error());
  }
  let mut capacity = usize::try_from(count).map_err(io::Error::other)?.saturating_add(1);
  let mut pids = loop {
    let mut pids = vec![0; capacity];
    let bytes = capacity
      .checked_mul(std::mem::size_of::<libc::pid_t>())
      .ok_or_else(|| io::Error::other("process list too large"))?;
    let bytes = libc::c_int::try_from(bytes).map_err(io::Error::other)?;
    // SAFETY: pids is writable for bytes bytes.
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if count < 0 {
      let error = io::Error::last_os_error();
      if error.raw_os_error() == Some(libc::ENOMEM) {
        capacity = capacity.checked_mul(2).ok_or_else(|| io::Error::other("process list too large"))?;
        continue;
      }
      return Err(error);
    }
    let count = usize::try_from(count).map_err(io::Error::other)?;
    if count >= capacity {
      capacity = capacity.checked_mul(2).ok_or_else(|| io::Error::other("process list too large"))?;
      continue;
    }
    pids.truncate(count);
    break pids;
  };
  pids.retain(|pid| {
    *pid > 0 && {
      // SAFETY: getsid reads the process table.
      unsafe { libc::getsid(*pid) == session }
    }
  });
  Ok(pids)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn session_members(session: libc::pid_t) -> io::Result<Vec<libc::pid_t>> {
  let mut pids = Vec::new();
  for entry in std::fs::read_dir("/proc")? {
    let entry = entry?;
    let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<libc::pid_t>().ok()) else {
      continue;
    };
    // SAFETY: getsid reads the process table.
    if pid > 0 && unsafe { libc::getsid(pid) } == session {
      pids.push(pid);
    }
  }
  Ok(pids)
}

#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct ProcessTree {
  handle: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl ProcessTree {
  pub(crate) fn new(child: &dyn portable_pty::Child) -> io::Result<Self> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::System::JobObjects::{
      AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
      JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    // SAFETY: null attributes and name create an anonymous job.
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
      return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateJobObjectW returned a new owned handle.
    let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw.cast()) };
    // SAFETY: all-zero is a valid initial value for this Win32 structure.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let size = u32::try_from(std::mem::size_of_val(&limits)).map_err(io::Error::other)?;
    // SAFETY: handle and structure are valid for the lifetime of this call.
    if unsafe {
      SetInformationJobObject(
        handle.as_raw_handle().cast(),
        JobObjectExtendedLimitInformation,
        std::ptr::from_ref(&limits).cast(),
        size,
      )
    } == 0
    {
      return Err(io::Error::last_os_error());
    }
    let process = child
      .as_raw_handle()
      .ok_or_else(|| io::Error::other("shell has no process handle"))?;
    // SAFETY: both handles are live and refer to a job and process respectively.
    if unsafe { AssignProcessToJobObject(handle.as_raw_handle().cast(), process.cast()) } == 0 {
      return Err(io::Error::last_os_error());
    }
    Ok(Self { handle })
  }

  pub(crate) fn terminate(&mut self) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    // SAFETY: handle owns a live job object.
    if unsafe { TerminateJobObject(self.handle.as_raw_handle().cast(), 1) } == 0 {
      return Err(io::Error::last_os_error());
    }
    Ok(())
  }
}
