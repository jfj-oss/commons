# megatron-bot

A Megatron member with its own account and devices. It is in the community like a person, its messages are end-to-end
encrypted like everyone's, and it answers only when @mentioned (or when someone replies to one of its messages). The
answers come from a model on this computer through [Ollama](http://127.0.0.1:11434); nothing said to it leaves the PC
unless a room's admin explicitly switches that room to DeepSeek.

## Set it up

```sh
cargo build --release -p commons-bot
install -Dm755 target/release/megatron-bot ~/.local/bin/megatron-bot

# 1. In Megatron, create an invite link for the community and send it to yourself.
# 2. Create the bot's account ("Oracle") and ask to join. An admin lets it in.
megatron-bot join 'commons://join/...'
```

The account's passphrase is generated on first run and kept in `~/.local/share/megatron-bot/passphrase` (mode 0600),
because an unattended service has nobody to type one. The bot's data lives in `~/.local/share/megatron-bot`.

When it is in, start it for good:

```sh
packaging/megatron-bot/enable.sh      # systemd user service, MemoryMax=1G; run this yourself
journalctl --user -u megatron-bot -f
```

```sh
systemctl --user stop megatron-bot    # or: disable --now
```

## Which model

With no configuration the bot uses `qwen3.5:9b` if Ollama has it, otherwise the largest other chat model installed
(embedding-only models are skipped):

```sh
ollama list
MEGATRON_BOT_MODEL=llama3.2:3b megatron-bot run
```

Ollama itself is never reconfigured: the bot does not unload models and does not pass `keep_alive`, so whatever the
owner has loaded stays as it is.

## DeepSeek (off by default, per room)

A community admin can switch one room:

* `@Oracle use deepseek` — answers in this room go to DeepSeek's servers, and each one is visibly labelled
  `(DeepSeek, leaves this chat)`.
* `@Oracle use local` — back to the model on this PC.

Only the exact phrase, only from an owner or admin, is obeyed. Put the key in `~/.config/deepseek-key` (or point
`MEGATRON_BOT_DEEPSEEK_KEY_FILE` elsewhere). A room's choice is remembered in `bot-state.json`.

## What it will not do

It has no tools and cannot run commands, read files or browse. Text in the chat cannot change its settings (only the
two admin phrases above, checked against the community's roles). Answers are capped in length, it posts at most
`MEGATRON_BOT_MAX_PER_MINUTE` messages a minute (6 by default), and each room has one answer at a time with a small
queue behind it. If Ollama is down it says so politely instead of going quiet or retrying forever.

## Environment

| variable | default |
| --- | --- |
| `MEGATRON_BOT_DATA` | `~/.local/share/megatron-bot` |
| `MEGATRON_BOT_NAME` | `Oracle` (the @mention) |
| `MEGATRON_BOT_MODEL` | unset: `qwen3.5:9b`, else the biggest chat model |
| `MEGATRON_BOT_OLLAMA_URL` | `http://127.0.0.1:11434` |
| `MEGATRON_BOT_DEEPSEEK_URL` | `https://api.deepseek.com` |
| `MEGATRON_BOT_DEEPSEEK_MODEL` | `deepseek-chat` |
| `MEGATRON_BOT_DEEPSEEK_KEY_FILE` | `~/.config/deepseek-key` |
| `MEGATRON_BOT_MAX_PER_MINUTE` | `6` |
| `MEGATRON_BOT_MAX_REPLY_CHARS` | `1200` |
| `MEGATRON_BOT_CONTEXT_MESSAGES` | `20` |
| `MEGATRON_BOT_LLM_TIMEOUT_SECS` | `120` |
