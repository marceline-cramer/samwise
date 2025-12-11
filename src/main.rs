use std::{
    process::{Command, Stdio},
    sync::mpsc::{Receiver, channel},
    time::Duration,
};

use anyhow::Context;
use discord_presence::models::Activity;
use rig::{
    client::{CompletionClient, Nothing},
    completion::Prompt,
    providers::ollama,
};
use serde::Deserialize;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone, Deserialize)]
pub struct Config {
    #[serde(with = "humantime_serde")]
    pub frequency: Duration,
    pub agent: AgentConfig,
    pub discord: DiscordConfig,
}

#[derive(Clone, Deserialize)]
pub struct DiscordConfig {
    pub client: u64,
}

#[derive(Clone, Deserialize)]
pub struct AgentConfig {
    pub model: String,
    pub preamble: String,
    pub prompt: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config_path = dirs::config_dir()
        .context("could not locate config path")?
        .join("samwise.toml");

    let config_src = std::fs::read_to_string(&config_path).context("failed to read config file")?;

    let config: Config = toml::from_str(&config_src).context("failed to parse config file")?;

    let (presence_tx, presence_rx) = channel();

    std::thread::spawn({
        let config = config.clone();
        move || rpc_thread(config, presence_rx)
    });

    let client: ollama::Client<reqwest::Client> =
        ollama::Client::new(Nothing).context("failed to create Ollama client")?;

    let mut last_diff = None;

    loop {
        let diff = get_diff().context("failed to get diff")?;

        if diff.is_empty() {
            presence_tx.send(None).unwrap();
            std::thread::sleep(config.frequency);
            continue;
        }

        if Some(&diff) == last_diff.as_ref() {
            std::thread::sleep(config.frequency);
            continue;
        }

        let agent = client
            .agent(&config.agent.model)
            .preamble(&config.agent.preamble)
            .context(&diff)
            .build();

        let mut response = agent
            .prompt(&config.agent.prompt)
            .await
            .context("failed to run prompt")?;

        // responses need to be at most 120 characters or setting activity fails
        response.truncate(120);

        let activity = Activity::new().details(&response);

        presence_tx.send(Some(activity)).unwrap();

        std::thread::sleep(config.frequency);

        last_diff = Some(diff);
    }
}

/// The Discord RPC needs to run its own thread because it uses crossbeam on
/// the inside. I'd love to write my own async bindings at some point but...
/// one thing at a time.
pub fn rpc_thread(config: Config, presence_rx: Receiver<Option<Activity>>) -> anyhow::Result<()> {
    let mut drpc = discord_presence::Client::new(config.discord.client);

    drpc.on_error(|ctx| {
        println!("RPC error: {:?}", ctx.event);
    })
    .persist();

    drpc.on_connected(|ctx| {
        println!("RPC connected: {:?}", ctx.event);
    })
    .persist();

    drpc.on_disconnected(|ctx| {
        println!("RPC disconnected: {:?}", ctx.event);
    })
    .persist();

    drpc.start();

    println!("waiting for Discord RPC...");

    drpc.block_until_event(discord_presence::Event::Ready)
        .context("failed to wait for ready state")?;

    println!("Discord RPC is ready.");

    while let Ok(activity) = presence_rx.recv() {
        match activity {
            Some(activity) => {
                drpc.set_activity(|_| activity)
                    .context("failed to set Discord activity")?;
            }
            None => {
                drpc.clear_activity()
                    .context("failed to clear Discord activity")?;
            }
        }
    }

    drpc.block_on().context("failed to join Discord RPC client")
}

pub fn get_diff() -> anyhow::Result<String> {
    Command::new("git")
        .arg("diff")
        .arg("--minimal")
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn git diff")?
        .wait_with_output()
        .context("failed to read git diff output")
        .and_then(|io| String::from_utf8(io.stdout).context("failed to parse git diff UTF-8"))
}
