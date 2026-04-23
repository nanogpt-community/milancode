# MilanCode

MilanCode is a rust based agentic coding harness for Nano-GPT - A clone of [pebble](https://github.com/nanogpt-community/pebble)

It supports:

- Nano-GPT

MilanCode is designed around an interactive REPL, local tools, managed sessions, MCP servers, and a user-controlled permission model. Web retrieval is provider-agnostic and always runs through Exa.

<img width="1571" height="504" alt="CleanShot 2026-04-21 at 21 54 52" src="https://github.com/user-attachments/assets/21eeb498-7f84-4c72-8fc2-718176ddf0ad" />

Check the [Changelog](CHANGELOG.md) for update/patch notes


## First-run setup

### 1. Save your Nano-GPT key

MilanCode can prompt you for credentials interactively:

```bash
milancode login
```

You can also target Nano-GPT directly:

```bash
milancode login nanogpt
```

Or pass the key inline:

```bash
milancode login nanogpt --api-key "$NANOGPT_API_KEY"
```

Inside the REPL, the equivalent commands are:

```text
/login
/auth
/login nanogpt
/logout nanogpt
```

If you run `/login` or `/auth` without a service, MilanCode opens a picker with:

- `nanogpt`
- `exa`

### 2. Save your Exa key

MilanCode uses Exa for all web search and scrape functionality.

Save it with:

```bash
milancode login exa
```

Or from inside the REPL:

```text
/login exa
/auth exa
```

You can also provide it inline:

```bash
milancode login exa --api-key "$EXA_API_KEY"
```

Or export it in your shell:

```bash
export EXA_API_KEY=...
```

## Daily usage

### Interactive REPL

Launch the REPL:

```bash
milancode
```

Useful first commands:

```text
/help
/status
/model
/login
/logout
/sessions
```

Basic prompt flow:

```text
> summarize this project
> inspect Cargo.toml and explain the workspace layout
> find the session restore logic
```

### One-shot prompt mode

For a single command without entering the REPL:

```bash
milancode prompt "Summarize this repository"
```

Or:

```bash
milancode "Inspect the current Rust workspace and explain the top-level crates"
```

### Restrict tool access

```bash
milancode --allowedTools read,glob "Summarize Cargo.toml"
```

## Core REPL commands

Common commands:

- `/help`
- `/help auth`
- `/help sessions`
- `/help extensions`
- `/help web`
- `/status`
- `/model`
- `/login`
- `/logout`
- `/provider`
- `/permissions`
- `/bypass`
- `/proxy`
- `/mcp`
- `/skills`
- `/plugins`
- `/sessions`
- `/resume`
- `/resume last`
- `/session switch <id>`

Notes:

- `/provider` only applies to NanoGPT-backed models.
- `Shift+Enter` and `Ctrl+J` insert a newline in the input editor.

## Authentication and config

MilanCode stores user config under:

```text
~/.milancode/
```

Credentials are stored in:

```text
~/.milancode/credentials.json
```

Possible stored keys:

- `nanogpt_api_key`
- `exa_api_key`

Environment variables still take precedence over saved credentials.

Useful environment variables:

- `NANOGPT_API_KEY`
- `EXA_API_KEY`
- `NANOGPT_BASE_URL`
- `EXA_BASE_URL`
- `MILANCODE_CONFIG_HOME`

`EXA_BASE_URL` defaults to `https://api.exa.ai`.

## Sessions and restore

MilanCode keeps managed sessions under:

```text
.milancode/sessions/
```

Useful flows:

- `/sessions` lists recent sessions
- `/resume` opens the picker
- `/resume last` restores the most recently modified session
- `/session switch <session-id>` switches inside the REPL
- `milancode resume [SESSION_ID_OR_PATH]` resumes from the CLI

Session restore includes more than just transcript history. MilanCode persists and restores:

- active model
- permission mode
- thinking toggle
- proxy tool-call toggle
- allowed tool set

That makes restored sessions behave much closer to the original live session.

## Permissions

MilanCode supports:

- `read-only`
- `workspace-write`
- `danger-full-access`

Examples:

```text
/permissions
/permissions workspace-write
/bypass
```

`/bypass` is a shortcut for `danger-full-access` in the current session.

## Web search and scrape

MilanCode keeps the tool names `WebSearch` and `WebScrape`, but both use Exa.

### WebSearch

- uses Exa `POST /search`
- defaults to Exa search type `auto`
- promotes to `deep` for deeper or more structured requests
- maps allowed and blocked domains into Exa domain filters

### WebScrape

- uses Exa `POST /contents`
- supports one or more URLs
- validates URLs before sending requests
- returns normalized previews in the TUI

### Check readiness

Run:

```text
/status
```

MilanCode reports Exa readiness separately from the active model backend.

## Extensions

MilanCode has three main extension surfaces:

- skills
- MCP servers
- plugins

### Skills

Create a project-local skill:

```text
/skills init my-skill
```

This creates:

```text
.milancode/skills/my-skill/SKILL.md
```

Useful commands:

```text
/skills
/skills help
```

### MCP servers

Create a starter MCP server entry:

```text
/mcp add my-server
```

This updates:

```text
.milancode/settings.json
```

Inspect what is configured:

```text
/mcp
/mcp tools
/mcp reload
```

Enable or disable a configured server locally:

```text
/mcp disable context7
/mcp enable context7
```

These local toggles are written to:

```text
.milancode/settings.local.json
```

That lets you keep a shared project MCP config while turning specific servers on or off per machine.

### Plugins

Useful commands:

```text
/plugins
/plugins help
/plugins install ./plugins/my-plugin
/plugins enable my-plugin-id
```

MilanCode expects plugins to expose:

```text
.milancode-plugin/plugin.json
```

## Proxy mode

MilanCode can run in XML proxy tool-call mode:

```text
/proxy status
/proxy on
/proxy off
```

When proxy mode is enabled, tool use is expected through XML `<tool_call>` blocks rather than native tool schemas.

## Troubleshooting

### A model won’t answer

- run `/status`
- confirm you saved credentials with `milancode login`
- or export the matching `*_API_KEY`
- verify the active model with `/model`

### Web tools are unavailable

- run `milancode login exa`
- or export `EXA_API_KEY`
- check `/status` for Exa readiness

### MCP server loads but shows no tools

- run `/mcp`
- run `/mcp tools`
- run `/mcp reload`
- check `.milancode/settings.json`
- check `.milancode/settings.local.json`
- if the server is marked `disabled`, run `/mcp enable <name>`

### Session restore feels wrong

- inspect `/status`
- use `/resume last` or `/session switch <id>`
- verify the session was saved after changing model, permissions, proxy, or thinking state

### Plugin setup is unclear

- run `/plugins help`
- confirm the plugin root contains `.milancode-plugin/plugin.json`

## Install, build, and development

### Build a release binary

```bash
cargo build --release -p milancode
```

Binary output:

```bash
./target/release/milancode
```

On Windows, the binary output is `target\\release\\milancode.exe`. MilanCode resolves config and
credentials from `MILANCODE_CONFIG_HOME` first, then `%USERPROFILE%\\.milancode` when `HOME` is not set.

### Run from source

```bash
cargo run -p milancode --
```

### Run tests

```bash
cargo test --workspace -- --test-threads=1
```

### Project config files

Common project-local files:

- `MILANCODE.md`
- `.milancode/settings.json`
- `.milancode/settings.local.json`
- `.milancode/skills/`
- `.milancode/sessions/`

### Release/update behavior

MilanCode’s self-update flow targets the GitHub releases for:

```text
nanogpt-community/milancode
```
