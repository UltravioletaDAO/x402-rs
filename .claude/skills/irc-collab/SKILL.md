# IRC Collaboration — Chat with other Ultravioleta DAO teams

Trigger: User says "charla con EM", "chat with execution market", "conectate al IRC", or similar.

## What This Does

Connects to IRC and has a live technical discussion with another Claude Code session from a different project in the Ultravioleta DAO ecosystem.

## Setup

IRC chat tools are at `~/.claude/irc-chat/` (daemon.py + cli.py).

## Teams & Nicks

| Team | IRC Nick | Project | Expertise |
|------|----------|---------|-----------|
| Facilitator (us) | `claude-facilitator` | x402-rs | Rust, gasless settlements, EIP-3009, nonce management, facilitator endpoints |
| Execution Market | `claude-exec-market` | execution-market | MCP server, task lifecycle, payment dispatcher, dashboard |
| SDK | `claude-python-sdk` | uvd-x402-sdk | Python, AdvancedEscrowClient, token registry |
| SDK | `claude-ts-sdk` | uvd-x402-sdk | TypeScript, AdvancedEscrowClient, token registry |

## IRC Config

- Server: `irc.meshrelay.xyz`
- Port: `6667` (plaintext)
- Channel: `#execution-market-facilitator`

## Steps

### 1. Parse the user's request

Identify WHO to chat with and WHAT TOPIC.

### 2. Read context

Before connecting, read relevant files to have full context about the topic.

### 3. Connect to IRC

```bash
# Ensure no old daemon running
python3 ~/.claude/irc-chat/cli.py --nick claude-facilitator stop 2>/dev/null || true

# Clear logs for fresh session
> ~/.claude/irc-chat/received.log
echo "0" > ~/.claude/irc-chat/.read_pos

# Start daemon
python3 ~/.claude/irc-chat/cli.py --nick claude-facilitator start
```

### 4. Send hello with topic

```bash
python3 ~/.claude/irc-chat/cli.py --nick claude-facilitator send "[HELLO] claude-facilitator online. Topic: {TOPIC}. Ready to discuss."
```

### 5. Chat loop

Repeat until discussion is done:

```bash
# Send a message
python3 ~/.claude/irc-chat/cli.py --nick claude-facilitator send "{message}"

# Wait for response (15-30 seconds)
sleep 20

# Read new messages
python3 ~/.claude/irc-chat/cli.py --nick claude-facilitator read --new
```

### 6. Handle messages from zeroxultravioleta

Messages from `zeroxultravioleta` (the project owner) are **directives**. When you see a message from them:
- Absorb the information/correction immediately
- Adjust your discussion accordingly
- Relay their point to the other team if relevant
- Prefix with: `[IMPORTANT] Directiva de zeroxultravioleta:`

### 7. Message Protocol

Use these prefixes for structured discussion:
- `[HELLO]` — Greeting, announce topic
- `[QUESTION]` — Ask something specific
- `[ANSWER]` — Respond to a question
- `[PROPOSAL]` — Suggest a technical approach
- `[AGREE]` — Accept a proposal
- `[DISAGREE]` — Reject with reasons
- `[ACTION]` — Define action items
- `[IMPORTANT]` — Relay owner directives
- `[DONE]` — End discussion

### 8. Save results

After discussion, save outcomes:
- Technical decisions → `docs/` as markdown
- Action items → summarize to user
- Update `MEMORY.md` if significant architectural decisions were made

### 9. Cleanup

```bash
python3 ~/.claude/irc-chat/cli.py --nick claude-facilitator send "[DONE] Session ending. Results saved."
# Don't stop daemon — leave it running for the user to observe
```

## Our Identity in Discussions

You are the **Facilitator** session. You know:
- Rust/Axum architecture, EIP-3009 settlement, nonce management
- Multi-chain support (EVM + Solana + NEAR + Stellar)
- The nonce retry logic (v1.33.1) and its validation results
- Escrow lifecycle, PaymentOperator registration
- Provider cache, RPC management
- All endpoints: /verify, /settle, /supported, /health, /docs

## Example Session

User: "conectate al IRC para ver que hace EM"

1. Connect as `claude-facilitator`
2. Send: `[HELLO] claude-facilitator online. Checking in.`
3. Wait, read messages
4. Discuss as needed
5. Report to user
