use std::process::{Command, Stdio};

use anyhow::Context;
use discord_presence::models::Activity;
use rig::{client::CompletionClient, completion::Prompt, providers::openai};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub openai: OpenAIConfig,
    pub agent: AgentConfig,
    pub discord: DiscordConfig,
}

#[derive(Deserialize)]
pub struct DiscordConfig {
    pub client: u64,
}

#[derive(Deserialize)]
pub struct OpenAIConfig {
    pub key: String,
    pub model: String,
}

#[derive(Deserialize)]
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

    let client = openai::Client::<reqwest::Client>::new(&config.openai.key)
        .context("failed to create OpenAI client")?;

    let diff = get_diff().context("failed to get diff")?;

    let agent = client
        .agent(&config.openai.model)
        .preamble(&config.agent.preamble)
        .context(&diff)
        .build();

    let response = agent
        .prompt(&config.agent.prompt)
        .await
        .context("failed to run prompt")?;

    println!("{response}");

    let mut drpc = discord_presence::Client::new(config.discord.client);

    drpc.start();

    drpc.block_until_event(discord_presence::Event::Ready)
        .context("failed to wait for Discord RPC ready")?;

    println!("RPC is ready");

    drpc.set_activity(|_| Activity::new().state("coding").details(&response))
        .context("failed to set Discord activity")?;

    println!("status is set");

    drpc.block_on()
        .context("failed to join Discord RPC client")?;

    Ok(())
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
