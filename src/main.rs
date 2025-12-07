use std::{
    process::{Command, Stdio},
    sync::mpsc::{Receiver, channel},
};

use anyhow::Context;
use discord_presence::models::Activity;
use rig::{client::CompletionClient, completion::Prompt, providers::openai};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Config {
    pub openai: OpenAIConfig,
    pub agent: AgentConfig,
    pub discord: DiscordConfig,
}

#[derive(Clone, Deserialize)]
pub struct DiscordConfig {
    pub client: u64,
}

#[derive(Clone, Deserialize)]
pub struct OpenAIConfig {
    pub key: String,
    pub model: String,
}

#[derive(Clone, Deserialize)]
pub struct AgentConfig {
    pub preamble: String,
    pub prompt: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = dirs::config_dir()
        .context("could not locate config path")?
        .join("samwise.toml");

    let config_src = std::fs::read_to_string(&config_path).context("failed to read config file")?;

    let config: Config = toml::from_str(&config_src).context("failed to parse config file")?;

    let (presence_tx, presence_rx) = channel();
    let rpc_thread = std::thread::spawn({
        let config = config.clone();
        move || rpc_thread(config, presence_rx)
    });

    let client = openai::Client::<reqwest::Client>::new(&config.openai.key)
        .context("failed to create OpenAI client")?;

    let diff = get_diff().context("failed to get diff")?;

    let agent = client
        .agent(&config.openai.model)
        .preamble(&config.agent.preamble)
        .context(&diff)
        .build();

    let mut response = agent
        .prompt(&config.agent.prompt)
        .await
        .context("failed to run prompt")?;

    // responses need to be at most 120 characters or setting activity fails
    response.truncate(128);

    let activity = Activity::new().details("coding").state(&response);

    presence_tx.send(activity).unwrap();

    drop(presence_tx);

    rpc_thread.join().unwrap()?;

    Ok(())
}

/// The Discord RPC needs to run its own thread because it uses crossbeam on
/// the inside. I'd love to write my own async bindings at some point but...
/// one thing at a time.
pub fn rpc_thread(config: Config, presence_rx: Receiver<Activity>) -> anyhow::Result<()> {
    let mut drpc = discord_presence::Client::new(config.discord.client);

    drpc.on_error(|ctx| {
        println!("RPC error: {:?}", ctx.event);
    })
    .persist();

    drpc.start();

    drpc.block_until_event(discord_presence::Event::Ready)
        .context("failed to wait for ready state")?;

    while let Ok(activity) = presence_rx.recv() {
        drpc.set_activity(|_| activity)
            .context("failed to set Discord activity")?;
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
