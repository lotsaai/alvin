//! A retained terminal and its gateway-owned shell.

use std::io;
use std::num::NonZeroU16;

use alvin_client::Client;
use alvin_protocol::{
  OpenTarget, TerminalAccepted, TerminalApiClient, TerminalEventBody, TerminalId, TerminalInput, TerminalResize, TerminalSize,
  TerminalSubscription,
};
use gpui_kit::base::async_util;
use gpui_kit::{
  AppContext as _, Context, Entity, EventEmitter, FocusHandle, IntoElement, ParentElement as _, Render, Styled as _, Subscription, Task,
  Window, prelude::FluentBuilder as _,
};

use crate::Error;
use crate::terminal::{
  Engine, EngineOptions, ExitStatus, StreamEvent, StreamSource, Terminal, TerminalConfig, TerminalEvent, TerminalState, Viewport,
};

pub(crate) enum TerminalPanelEvent {
  Closed,
}

impl EventEmitter<TerminalPanelEvent> for TerminalPanel {}
/// A terminal's process and transport lifecycle.
enum Lifecycle {
  Starting,
  Running,
  Disconnected,
  Exited,
  Failed(Error),
  Closing,
  Closed,
}

pub(crate) struct TerminalPanel {
  terminal: Entity<TerminalState>,
  client: Client,
  terminal_id: Option<TerminalId>,
  lifecycle: Lifecycle,
  output: Option<Task<()>>,
  input: Option<Task<()>>,
  close_waiters: Vec<async_util::Sender<()>>,
  _subscription: Subscription,
}

impl TerminalPanel {
  pub(crate) fn new(
    client: Client,
    target: OpenTarget,
    settings: &crate::TerminalSettings,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let (source, mut peer) = StreamSource::new();
    let events = peer.take_events();
    let release_client = client.clone();
    cx.on_release(move |panel: &mut Self, cx| {
      let thread = panel.terminal.update(cx, |terminal, cx| {
        terminal.close(cx);
        terminal.take_thread()
      });
      if let Some(thread) = thread {
        cx.background_spawn(async move {
          if thread.join().is_err() {
            tracing::error!("terminal engine thread panicked");
          }
        })
        .detach();
      }
      if let Some(terminal) = panel.terminal_id.filter(|_| !matches!(panel.lifecycle, Lifecycle::Closed)) {
        cx.spawn(async move |_| {
          if let Err(error) = TerminalApiClient::close(&release_client, terminal).await {
            tracing::error!(%error, "closing terminal on release");
          }
        })
        .detach();
      }
    })
    .detach();
    let terminal = cx.new(|cx| {
      TerminalState::new(
        Engine::new(source, EngineOptions::default()),
        TerminalConfig::from(settings),
        window,
        cx,
      )
    });
    let subscription = cx.subscribe(&terminal, |this, _, event: &TerminalEvent, cx| {
      if matches!(this.lifecycle, Lifecycle::Closing | Lifecycle::Closed) {
        return;
      }
      match event {
        TerminalEvent::InputRejected(source) => {
          this.lifecycle = Lifecycle::Failed(Error::TerminalInput(Box::new(*source)));
          cx.notify();
        }
        TerminalEvent::Failed(source) => {
          this.lifecycle = Lifecycle::Failed(Error::TerminalEngine(io::Error::other(source.clone())));
          cx.notify();
        }
        _ => {}
      }
    });
    let (started, wait_started) = async_util::unbounded::<(TerminalId, crate::terminal::StreamEvents)>();
    let input_client = client.clone();
    let panel_client = client.clone();
    let input = cx.spawn(async move |this, cx| {
      let (terminal, input_events) = match wait_started.recv().await {
        Ok(started) => started,
        Err(source) => {
          tracing::debug!(%source, "terminal input stopped before start");
          return;
        }
      };
      loop {
        let event = match input_events.recv().await {
          Ok(event) => event,
          Err(source) => {
            if this
              .update(cx, |panel, cx| {
                if !matches!(panel.lifecycle, Lifecycle::Exited | Lifecycle::Closing | Lifecycle::Closed) {
                  panel.lifecycle = Lifecycle::Failed(Error::TerminalEngine(source));
                  cx.notify();
                }
              })
              .is_err()
            {
              tracing::debug!("terminal panel was released before input stopped");
            }
            return;
          }
        };
        let result = match event {
          StreamEvent::Input(bytes) => TerminalApiClient::input(&input_client, TerminalInput { terminal, bytes }).await,
          StreamEvent::Resize(viewport) => match Self::size(viewport) {
            Ok(size) => TerminalApiClient::resize(&input_client, TerminalResize { terminal, size }).await,
            Err(error) => {
              Self::failed(&this, cx, error);
              return;
            }
          },
          StreamEvent::Closed => return,
        };
        if let Err(error) = result {
          Self::failed(&this, cx, Error::Gateway(error.into()));
          return;
        }
      }
    });
    let output = cx.spawn(async move |this, cx| {
      let events = match events {
        Ok(events) => events,
        Err(source) => {
          Self::failed(&this, cx, Error::TerminalEngine(source));
          return;
        }
      };
      let viewport = match events.recv().await {
        Ok(StreamEvent::Resize(viewport)) => viewport,
        Ok(StreamEvent::Closed) => {
          if this
            .update(cx, |panel, cx| {
              panel.lifecycle = Lifecycle::Closed;
              cx.emit(TerminalPanelEvent::Closed);
              cx.notify();
            })
            .is_err()
          {
            tracing::debug!("terminal panel was released while closing");
          }
          return;
        }
        Ok(StreamEvent::Input(_)) => {
          Self::failed(&this, cx, Self::engine_error("terminal received input before measurement"));
          return;
        }
        Err(source) => {
          Self::failed(&this, cx, Error::TerminalEngine(source));
          return;
        }
      };
      let mut events = Some(events);
      let size = match Self::size(viewport) {
        Ok(size) => size,
        Err(error) => {
          Self::failed(&this, cx, error);
          return;
        }
      };
      let mut subscription = match TerminalApiClient::terminal_events(&client, TerminalSubscription::Open { target, size }).await {
        Ok(subscription) => subscription,
        Err(error) => {
          Self::failed(&this, cx, Error::Gateway(error.into()));
          return;
        }
      };
      let mut terminal = None;
      let mut last = None;
      let mut connection = client.watch_connection();
      loop {
        match subscription.next().await {
          Some(Ok(event)) => {
            let exited = matches!(&event.body, TerminalEventBody::Exited { .. });
            match event.body {
              TerminalEventBody::Started => {
                if terminal.replace(event.terminal).is_some() {
                  Self::failed(&this, cx, Self::engine_error("terminal started twice"));
                  return;
                }
                if this
                  .update(cx, |panel, cx| {
                    panel.terminal_id = Some(event.terminal);
                    cx.notify();
                  })
                  .is_err()
                {
                  if let Err(error) = TerminalApiClient::close(&client, event.terminal).await {
                    tracing::error!(%error, "closing terminal after panel release");
                  }
                  return;
                }
              }
              TerminalEventBody::Output { bytes } => {
                if let Err(source) = peer.output(bytes).await {
                  Self::failed(&this, cx, Error::TerminalEngine(source));
                  return;
                }
              }
              TerminalEventBody::Exited { code, signal } => {
                let code = match code.map(i32::try_from).transpose() {
                  Ok(code) => code,
                  Err(source) => {
                    Self::failed(&this, cx, Error::TerminalEngine(io::Error::other(source)));
                    return;
                  }
                };
                peer.exit(ExitStatus::new(code, signal));
                if this
                  .update(cx, |panel, cx| {
                    panel.lifecycle = Lifecycle::Exited;
                    cx.notify();
                  })
                  .is_err()
                {
                  return;
                }
              }
              TerminalEventBody::Failed { code, message } => {
                let source = match alvin_protocol::ErrorCode::from_code(code) {
                  Some(code) => alvin_client::Error::Gateway { code, message, data: None },
                  None => alvin_client::Error::UnknownCode { code, message },
                };
                Self::failed(&this, cx, Error::Gateway(source));
                return;
              }
            }
            last = Some(event.id);
            match TerminalApiClient::accepted(
              &client,
              TerminalAccepted {
                terminal: event.terminal,
                through: event.id,
              },
            )
            .await
            {
              Ok(()) => {
                if let Some(events) = events.take() {
                  if started.send((event.terminal, events)).await.is_err() {
                    if let Err(error) = TerminalApiClient::close(&client, event.terminal).await {
                      tracing::error!(%error, "closing terminal without input receiver");
                    }
                    return;
                  }
                  if this
                    .update(cx, |panel, cx| {
                      panel.lifecycle = Lifecycle::Running;
                      cx.notify();
                    })
                    .is_err()
                  {
                    return;
                  }
                }
                if exited {
                  return;
                }
                continue;
              }
              Err(error) => match alvin_client::Error::from(error) {
                alvin_client::Error::Disconnected(_) if !exited => {}
                alvin_client::Error::Disconnected(error) if exited => {
                  tracing::debug!(%error, "terminal exit acknowledgement was disconnected");
                  return;
                }
                error => {
                  Self::failed(&this, cx, Error::Gateway(error));
                  return;
                }
              },
            }
          }
          Some(Err(error)) => {
            Self::failed(&this, cx, Error::Gateway(alvin_client::Error::Response(error)));
            return;
          }
          None => {}
        }
        let Some(id) = terminal else {
          Self::failed(&this, cx, Self::engine_error("terminal stream ended before start"));
          return;
        };
        if this
          .update(cx, |panel, cx| {
            if matches!(panel.lifecycle, Lifecycle::Running | Lifecycle::Starting) {
              panel.lifecycle = Lifecycle::Disconnected;
              cx.notify();
            }
          })
          .is_err()
        {
          return;
        }
        loop {
          let state = *connection.borrow_and_update();
          match state {
            alvin_client::ConnectionState::Connected => {
              match TerminalApiClient::terminal_events(&client, TerminalSubscription::Resume { terminal: id, after: last }).await {
                Ok(next) => {
                  subscription = next;
                  if let Some(events) = events.take()
                    && started.send((id, events)).await.is_err()
                  {
                    if let Err(error) = TerminalApiClient::close(&client, id).await {
                      tracing::error!(%error, "closing resumed terminal without input receiver");
                    }
                    return;
                  }
                  if this
                    .update(cx, |panel, cx| {
                      if matches!(panel.lifecycle, Lifecycle::Disconnected) {
                        panel.lifecycle = Lifecycle::Running;
                        cx.notify();
                      }
                    })
                    .is_err()
                  {
                    return;
                  }
                  break;
                }
                Err(error) => match alvin_client::Error::from(error) {
                  alvin_client::Error::Disconnected(_) => {}
                  error => {
                    Self::failed(&this, cx, Error::Gateway(error));
                    return;
                  }
                },
              }
            }
            alvin_client::ConnectionState::Lost => {
              Self::failed(&this, cx, Self::engine_error("terminal connection was lost"));
              return;
            }
            alvin_client::ConnectionState::Reconnecting { .. } => {}
          }
          if connection.changed().await.is_err() {
            Self::failed(&this, cx, Self::engine_error("terminal connection state ended"));
            return;
          }
        }
      }
    });
    Self {
      terminal,
      client: panel_client,
      terminal_id: None,
      lifecycle: Lifecycle::Starting,
      output: Some(output),
      input: Some(input),
      close_waiters: Vec::new(),
      _subscription: subscription,
    }
  }

  fn size(viewport: Viewport) -> crate::Result<TerminalSize> {
    let columns = NonZeroU16::new(viewport.columns()).ok_or_else(|| Self::engine_error("terminal has no columns"))?;
    let rows = NonZeroU16::new(viewport.rows()).ok_or_else(|| Self::engine_error("terminal has no rows"))?;
    let pixel_width = u16::try_from(viewport.cell_width().saturating_mul(u32::from(viewport.columns()))).unwrap_or(u16::MAX);
    let pixel_height = u16::try_from(viewport.cell_height().saturating_mul(u32::from(viewport.rows()))).unwrap_or(u16::MAX);
    Ok(TerminalSize {
      columns,
      rows,
      pixel_width,
      pixel_height,
    })
  }

  fn engine_error(message: &'static str) -> Error {
    Error::TerminalEngine(io::Error::new(io::ErrorKind::BrokenPipe, message))
  }

  fn failed(this: &gpui_kit::WeakEntity<Self>, cx: &mut gpui_kit::AsyncApp, error: Error) {
    if this
      .update(cx, |panel, cx| {
        if !matches!(panel.lifecycle, Lifecycle::Closed) {
          panel.lifecycle = Lifecycle::Failed(error);
          cx.notify();
        }
      })
      .is_err()
    {
      tracing::debug!("terminal panel was released before reporting an error");
    }
  }

  pub(crate) fn focus_handle(&self, cx: &gpui_kit::App) -> FocusHandle {
    self.terminal.read(cx).focus_handle().clone()
  }

  pub(crate) fn set_terminal_settings(&mut self, settings: &crate::TerminalSettings, cx: &mut Context<Self>) {
    self
      .terminal
      .update(cx, |terminal, cx| terminal.set_terminal_settings(settings, cx));
  }

  #[cfg(test)]
  pub(crate) fn test_state(&self) -> Entity<TerminalState> {
    self.terminal.clone()
  }

  #[cfg(test)]
  pub(crate) fn test_ready(&self) -> Option<std::result::Result<(), String>> {
    match &self.lifecycle {
      Lifecycle::Running => Some(Ok(())),
      Lifecycle::Failed(error) => Some(Err(error.to_string())),
      _ => None,
    }
  }

  pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
    self.terminal.update(cx, |terminal, cx| terminal.set_visible(visible, cx));
  }

  pub(crate) fn close_and_wait(&mut self, cx: &mut Context<Self>) -> async_util::Receiver<()> {
    let (sender, receiver) = async_util::unbounded();
    if matches!(self.lifecycle, Lifecycle::Closed) {
      if let Err(error) = sender.try_send(()) {
        tracing::debug!(%error, "terminal close waiter was dropped");
      }
    } else {
      self.close_waiters.push(sender);
      self.close(cx);
    }
    receiver
  }

  pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
    if matches!(self.lifecycle, Lifecycle::Closing | Lifecycle::Closed) {
      return;
    }
    self.lifecycle = Lifecycle::Closing;
    self.input.take();
    self.output.take();
    let thread = self.terminal.update(cx, |terminal, cx| {
      terminal.close(cx);
      terminal.take_thread()
    });
    let client = self.client.clone();
    let terminal = self.terminal_id;
    cx.spawn(async move |this, cx| {
      let joined = if let Some(thread) = thread {
        cx.background_spawn(async move { thread.join() })
          .await
          .map_err(|_| Error::TerminalThread)
      } else {
        Ok(())
      };
      let closed = if let Some(terminal) = terminal {
        match TerminalApiClient::close(&client, terminal).await {
          Ok(()) => Ok(()),
          Err(error) => {
            let error = alvin_client::Error::from(error);
            if matches!(
              error,
              alvin_client::Error::Gateway {
                code: alvin_protocol::ErrorCode::UnknownTerminal,
                ..
              }
            ) {
              Ok(())
            } else {
              Err(Error::Gateway(error))
            }
          }
        }
      } else {
        Ok(())
      };
      if this
        .update(cx, |panel, cx| {
          match joined.and(closed) {
            Ok(()) => {
              panel.lifecycle = Lifecycle::Closed;
              cx.emit(TerminalPanelEvent::Closed);
            }
            Err(error) => {
              tracing::error!(%error, "closing terminal");
              panel.lifecycle = Lifecycle::Failed(error);
            }
          }
          for waiter in panel.close_waiters.drain(..) {
            if let Err(error) = waiter.try_send(()) {
              tracing::debug!(%error, "terminal close waiter was dropped");
            }
          }
          cx.notify();
        })
        .is_err()
      {
        tracing::debug!("terminal panel was released while closing");
      }
    })
    .detach();
    cx.notify();
  }
}

impl Render for TerminalPanel {
  fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    let status = match &self.lifecycle {
      Lifecycle::Starting => Some("Starting terminal".to_owned()),
      Lifecycle::Failed(error) => Some(error.to_string()),
      Lifecycle::Closing => Some("Closing terminal".to_owned()),
      Lifecycle::Closed => Some("Terminal closed".to_owned()),
      Lifecycle::Running => None,
      Lifecycle::Disconnected => Some("Terminal disconnected; reconnecting".to_owned()),
      Lifecycle::Exited => Some("Terminal exited".to_owned()),
    };
    gpui_kit::div()
      .size_full()
      .flex()
      .flex_col()
      .child(Terminal::new("terminal", &self.terminal).flex_1().min_h_0())
      .when_some(status, |view, status| view.child(gpui_kit::div().text_sm().child(status)))
  }
}
