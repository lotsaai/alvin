//! Records conversation fixtures from real omp through the gateway, for `alvin-conversation`'s tests:
//! `mise run generate:conversation-fixtures`. Each scenario writes `<name>.events.json` (every event of the live session
//! up to its settle) and `<name>.messages.json` (the session's `live/messages` afterwards).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use alvin_client::{ChannelPair, Client, Endpoint};
use alvin_gateway::{Gateway, GatewayConfig};
use alvin_omp_host::HostConfig;
use alvin_protocol::{
  AnswersUiResponse, AskAnswer, DialogApiClient, EntriesRequest, Event, EventBody, EventId, ExtensionUiRequest, ExtensionUiResponse,
  LiveApiClient, MessagesRequest, OpenRequest, OpenTarget, ProjectsApiClient, PromptRequest, RespondRequest, RpcAgentEvent,
  SubscribeRequest,
};
use eyre::{Result, eyre};

const MODEL: &str = "zai/glm-5.3-flash";
const PATIENCE: Duration = Duration::from_secs(180);
/// The records of one real omp run, used only so the temporary repository is a listed project.
const SEED_RECORDS: &str = include_str!("../fixtures/session-records.jsonl");
const SEED_CWD: &str = "/tmp/alvin-probe.hnBA/proj";

struct Scenario {
  name: &'static str,
  /// Extra omp flags after the common ones.
  flags: &'static [&'static str],
  prompt: &'static str,
  /// Abort the turn once the assistant starts streaming.
  abort: bool,
  /// A follow-up queued once the assistant starts streaming.
  follow_up: Option<&'static str>,
  /// Answer an `ask` dialog with its first option.
  answer_asks: bool,
  /// A second prompt, sent once the first turn settles. The entries are recorded before it, after it, and as the delta between.
  second_prompt: Option<&'static str>,
  /// Write the recording for the UI crate's tests instead of the conversation crate's.
  for_ui: bool,
  /// After the turns, label an entry, clear the label and navigate to a user message, recording the entries after each.
  tree_actions: bool,
}

const SCENARIOS: [Scenario; 8] = [
  Scenario {
    name: "plain_turn",
    flags: &["--no-tools"],
    prompt: "Reply with the single word ok.",
    abort: false,
    follow_up: None,
    answer_asks: false,
    second_prompt: None,
    tree_actions: false,
    for_ui: false,
  },
  Scenario {
    name: "tool_turn",
    flags: &["--approval-mode", "yolo"],
    prompt: "Use your bash tool to run `echo alvin-tool-ok`, then reply with the single word done.",
    abort: false,
    follow_up: None,
    answer_asks: false,
    second_prompt: None,
    tree_actions: false,
    for_ui: false,
  },
  Scenario {
    name: "aborted_turn",
    flags: &["--no-tools"],
    prompt: "Write the numbers from 1 to 400, one per line, nothing else.",
    abort: true,
    follow_up: None,
    answer_asks: false,
    second_prompt: None,
    tree_actions: false,
    for_ui: false,
  },
  Scenario {
    name: "queue_turn",
    flags: &["--no-tools"],
    prompt: "Write the numbers from 1 to 40, one per line, nothing else.",
    abort: false,
    follow_up: Some("Now reply with the single word done."),
    answer_asks: false,
    second_prompt: None,
    tree_actions: false,
    for_ui: false,
  },
  Scenario {
    name: "ask_turn",
    flags: &["--tools", "ask"],
    prompt: "Use the ask tool exactly once to ask me which colour I prefer, with the options red and blue. Then reply with the single word I chose.",
    abort: false,
    follow_up: None,
    answer_asks: true,
    second_prompt: None,
    tree_actions: false,
    for_ui: false,
  },
  Scenario {
    name: "tree_turn",
    flags: &["--no-tools"],
    prompt: "Reply with the single word ok.",
    abort: false,
    follow_up: None,
    answer_asks: false,
    second_prompt: Some("Reply with the single word again."),
    tree_actions: true,
    for_ui: false,
  },
  Scenario {
    name: "tools_turn",
    flags: &["--approval-mode", "yolo", "--tools", "read,write,edit,grep,glob,bash"],
    prompt: "Do these in order with your tools, one call each, and then reply with the single word done: \
             write a file notes.txt containing exactly the two lines `alpha` and `beta`; read notes.txt; \
             edit notes.txt so `beta` becomes `gamma`; grep for `gamma` in the current directory; \
             glob for `*.txt`; run `echo alvin-bash` with bash.",
    abort: false,
    follow_up: None,
    answer_asks: false,
    second_prompt: None,
    tree_actions: false,
    for_ui: true,
  },
  Scenario {
    name: "task_turn",
    flags: &["--approval-mode", "yolo", "--tools", "task,read"],
    prompt: "Use the task tool exactly once to launch one subagent that reads the file Cargo.toml if it exists and says in one \
             sentence what it found, then reply with the single word done.",
    abort: false,
    follow_up: None,
    answer_asks: false,
    second_prompt: None,
    tree_actions: false,
    for_ui: true,
  },
];

#[tokio::main]
async fn main() -> Result<()> {
  let mut args = std::env::args().skip(1);
  let usage = || eyre!("usage: record_conversation <conversation fixtures dir> <ui fixtures dir>");
  let conversation = PathBuf::from(args.next().ok_or_else(usage)?);
  let ui = PathBuf::from(args.next().ok_or_else(usage)?);
  std::fs::create_dir_all(&conversation)?;
  std::fs::create_dir_all(&ui)?;
  for scenario in &SCENARIOS {
    record(scenario, if scenario.for_ui { &ui } else { &conversation }).await?;
    println!("recorded {}", scenario.name);
  }
  Ok(())
}

async fn record(scenario: &Scenario, out: &Path) -> Result<()> {
  let base = std::env::temp_dir().join(format!("alvin-record-{}", EventId::new().as_uuid()));
  let result = run(scenario, out, &base).await;
  let removed = std::fs::remove_dir_all(&base);
  // A failed recording says why before a failed cleanup does.
  result?;
  Ok(removed?)
}

/// Labels the first user message, clears the label, then navigates to the second user message. Each step is the action's
/// name, the entries appended since the step before it (`delta`), and every entry after it (`after`).
async fn record_tree_actions(
  client: &Client,
  live: alvin_protocol::LiveSessionId,
  mut previous: alvin_protocol::HistoryEntries,
) -> Result<Vec<serde_json::Value>> {
  let candidates = LiveApiClient::branch_messages(client, live).await?;
  let first = candidates.first().ok_or_else(|| eyre!("no user message to label"))?;
  let second = candidates.get(1).ok_or_else(|| eyre!("no second user message"))?;
  let first = alvin_protocol::EntryId::from(first.entry_id.clone());
  let second = alvin_protocol::EntryId::from(second.entry_id.clone());
  let mut steps = Vec::new();
  for name in ["label", "clear_label", "navigate"] {
    match name {
      "label" => {
        LiveApiClient::label(
          client,
          alvin_protocol::LabelRequest {
            live,
            entry: first.clone(),
            label: Some("start".to_owned()),
          },
        )
        .await?;
      }
      "clear_label" => {
        LiveApiClient::label(
          client,
          alvin_protocol::LabelRequest {
            live,
            entry: first.clone(),
            label: None,
          },
        )
        .await?;
      }
      _ => {
        LiveApiClient::navigate_tree(
          client,
          alvin_protocol::NavigateRequest {
            live,
            entry: second.clone(),
            summarize: None,
            custom_instructions: None,
          },
        )
        .await?;
      }
    }
    let since = previous.entries.last().map(|entry| entry.id.clone());
    let delta = LiveApiClient::entries(client, EntriesRequest { live, since }).await?;
    let after = LiveApiClient::entries(client, EntriesRequest { live, since: None }).await?;
    previous = after.clone();
    steps.push(serde_json::json!({ "name": name, "delta": delta, "after": after }));
  }
  Ok(steps)
}

async fn run(scenario: &Scenario, out: &Path, base: &Path) -> Result<()> {
  let base = {
    std::fs::create_dir_all(base)?;
    base.canonicalize()?
  };
  let repo = base.join("repo");
  let sessions = base.join("sessions");
  let home = base.join("home");
  let session_dir = sessions.join("-repo");
  for dir in [&repo, &session_dir, &home] {
    std::fs::create_dir_all(dir)?;
  }
  let git = Command::new("git").args(["init", "--quiet"]).current_dir(&repo).output()?;
  eyre::ensure!(git.status.success(), "git init failed");
  let seed = SEED_RECORDS.replace(SEED_CWD, &repo.display().to_string());
  std::fs::write(session_dir.join("2026-01-01T00-00-00-000Z_seed.jsonl"), seed)?;

  let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
    .map(PathBuf::from)
    .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".omp/agent")))
    .ok_or_else(|| eyre!("cannot find omp's agent directory"))?;
  let mut args: Vec<OsString> = [
    "--no-skills",
    "--no-rules",
    "--no-extensions",
    "--no-context-files",
    "--session-dir",
  ]
  .map(OsString::from)
  .into();
  args.push(session_dir.into_os_string());
  args.extend(scenario.flags.iter().map(OsString::from));
  let omp = HostConfig::builder()
    .omp(PathBuf::from("omp"))
    .extra_args(args)
    .env(vec![
      (OsString::from("HOME"), home.into_os_string()),
      (OsString::from("PI_CODING_AGENT_DIR"), agent_dir.into_os_string()),
    ])
    .build();
  let gateway = Gateway::new(
    GatewayConfig::builder()
      .sessions_dir(sessions)
      .omp(omp)
      .default_model(Some(MODEL.to_owned()))
      .build(),
  );
  let client = Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(16)?))).await?;

  let projects = ProjectsApiClient::list(&client).await?;
  let project = projects.first().ok_or_else(|| eyre!("no project listed"))?;
  let checkout = project.checkouts.first().ok_or_else(|| eyre!("project has no checkout"))?;
  let opened = LiveApiClient::open(
    &client,
    OpenRequest {
      target: OpenTarget::New {
        project: project.id.clone(),
        checkout: checkout.id.clone(),
      },
      model: None,
    },
  )
  .await?;
  let mut stream = client
    .events(SubscribeRequest {
      live: opened.live,
      after: None,
    })
    .await?;
  LiveApiClient::prompt(
    &client,
    PromptRequest {
      live: opened.live,
      message: scenario.prompt.to_owned(),
    },
  )
  .await?;

  let mut events: Vec<Event> = Vec::new();
  let mut aborted = false;
  let mut queued = false;
  let mut second_sent = false;
  // A turn can announce its settling twice (the prompt result, then the session event): count prompts, not settles.
  let mut prompt_results = 0;
  let wanted = 1 + usize::from(scenario.second_prompt.is_some());
  let mut entries_before = None;
  loop {
    let event = tokio::time::timeout(PATIENCE, stream.next())
      .await?
      .ok_or_else(|| eyre!("the event stream ended"))??;
    let settled = match &event.body {
      EventBody::SessionSettled(_) => true,
      EventBody::PromptResult(result) => result.session_settled,
      _ => false,
    };
    let streaming = matches!(&event.body, EventBody::Agent(RpcAgentEvent::MessageUpdate(_)));
    if let (true, EventBody::ExtensionUiRequest(ExtensionUiRequest::Ask(ask))) = (scenario.answer_asks, &event.body) {
      let question = ask.questions.first().ok_or_else(|| eyre!("an ask without questions"))?;
      let option = question.options.first().ok_or_else(|| eyre!("an ask question without options"))?;
      let answers = vec![AskAnswer {
        id: question.id.clone(),
        selected_options: vec![option.label.clone()],
        custom_input: None,
      }];
      DialogApiClient::respond(
        &client,
        RespondRequest {
          live: opened.live,
          response: ExtensionUiResponse::AnswersUiResponse(AnswersUiResponse {
            id: ask.id.clone(),
            answers,
          }),
        },
      )
      .await?;
    }
    if matches!(&event.body, EventBody::PromptResult(_)) {
      prompt_results += 1;
    }
    events.push(event);
    if let Some(message) = scenario.follow_up
      && streaming
      && !queued
    {
      queued = true;
      LiveApiClient::follow_up(
        &client,
        PromptRequest {
          live: opened.live,
          message: message.to_owned(),
        },
      )
      .await?;
    }
    if scenario.abort && streaming && !aborted {
      aborted = true;
      LiveApiClient::abort(&client, opened.live).await?;
    }
    if settled {
      if let (Some(second), false, true) = (scenario.second_prompt, second_sent, prompt_results >= 1) {
        second_sent = true;
        entries_before = Some(
          LiveApiClient::entries(
            &client,
            EntriesRequest {
              live: opened.live,
              since: None,
            },
          )
          .await?,
        );
        LiveApiClient::prompt(
          &client,
          PromptRequest {
            live: opened.live,
            message: second.to_owned(),
          },
        )
        .await?;
        continue;
      }
      if prompt_results >= wanted {
        break;
      }
    }
  }

  // omp refuses to page messages for a moment after a turn settles.
  tokio::time::sleep(Duration::from_secs(1)).await;
  let mut messages = Vec::new();
  let mut cursor = None;
  let total = loop {
    let page = LiveApiClient::messages(
      &client,
      MessagesRequest {
        live: opened.live,
        cursor,
        limit: None,
      },
    )
    .await?;
    messages.extend(page.messages);
    match page.next_cursor {
      Some(next) => cursor = Some(next),
      None => break page.total_messages,
    }
  };
  eyre::ensure!(
    usize::try_from(total)? == messages.len(),
    "live/messages ended before its advertised total"
  );

  std::fs::write(
    out.join(format!("{}.events.json", scenario.name)),
    serde_json::to_string_pretty(&events)?,
  )?;
  let page = alvin_protocol::MessagesPage {
    messages,
    total_messages: total,
    next_cursor: None,
  };
  std::fs::write(
    out.join(format!("{}.messages.json", scenario.name)),
    serde_json::to_string_pretty(&page)?,
  )?;

  if let Some(before) = entries_before {
    let since = before.entries.last().map(|entry| entry.id.clone());
    let after = LiveApiClient::entries(
      &client,
      EntriesRequest {
        live: opened.live,
        since: None,
      },
    )
    .await?;
    let delta = LiveApiClient::entries(&client, EntriesRequest { live: opened.live, since }).await?;
    if scenario.tree_actions {
      let steps = record_tree_actions(&client, opened.live, after.clone()).await?;
      std::fs::write(
        out.join(format!("{}.steps.json", scenario.name)),
        serde_json::to_string_pretty(&steps)?,
      )?;
    }
    for (name, entries) in [("before", &before), ("after", &after), ("delta", &delta)] {
      std::fs::write(
        out.join(format!("{}.entries_{name}.json", scenario.name)),
        serde_json::to_string_pretty(entries)?,
      )?;
    }
  }

  drop(stream);
  LiveApiClient::close(&client, opened.live).await?;
  gateway.shutdown().await?;
  Ok(())
}
