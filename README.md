# Bacchus

AST-aware coordination CLI for multi-agent work on codebases.

Bacchus helps AI agents coordinate when working on the same codebase by tracking:
- **Task ownership** - who's working on what
- **Symbol conflicts** - detecting when agents modify overlapping code
- **Breaking changes** - notifying stakeholders of API changes
- **Human escalation** - routing decisions that need human input

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/vu1n/bacchus.git
cd bacchus

# Build release binary
cargo build --release

# Install to PATH (optional)
cp target/release/bacchus ~/.local/bin/
```

### One-liner Install

```bash
curl -sSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/install.sh | bash
```

This will download a pre-built binary if available, or build from source as a fallback.

## Quick Start

```bash
# Initialize bacchus in your project
mkdir -p .bacchus

# Index your codebase
bacchus index src

# Claim a task
bacchus claim TASK-1 agent-a

# Declare what you plan to modify
bacchus workplan TASK-1 agent-a --modifies-symbols "MyClass::method"

# Keep your task alive (run every 5 minutes)
bacchus heartbeat TASK-1 agent-a

# Release when done
bacchus release TASK-1 agent-a
```

## Commands

### Coordination

| Command | Description |
|---------|-------------|
| `claim <bead_id> <agent_id>` | Claim ownership of a task |
| `release <bead_id> <agent_id>` | Release a claimed task |
| `workplan <bead_id> <agent_id> [opts]` | Declare symbols you plan to modify |
| `footprint <bead_id> <agent_id> --files <files>` | Report actual changes made |
| `heartbeat <bead_id> <agent_id>` | Keep task alive, get notifications |
| `stale [--minutes N]` | Find abandoned tasks |

### Symbols

| Command | Description |
|---------|-------------|
| `index <path>` | Index files/directories for symbol tracking |
| `symbols [--pattern X] [--kind Y]` | Search for symbols |
| `context <bead_id>` | Get code context for a task |

### Communication

| Command | Description |
|---------|-------------|
| `notifications <agent_id>` | Get pending notifications |
| `resolve <id> <agent_id> <action>` | Acknowledge/resolve a notification |
| `stakeholders <symbol>` | Find who cares about a symbol |
| `notify <symbol> <agent_id> <bead_id> <kind> <desc>` | Notify stakeholders of a change |

### Human Escalation

| Command | Description |
|---------|-------------|
| `decide <agent_id> <bead_id> <question> --options "A,B,C"` | Request human decision |
| `answer <id> <human_id> <decision>` | Submit a human decision |
| `pending` | Get pending human decisions |

### Info

| Command | Description |
|---------|-------------|
| `status` | Show current tasks and notifications |
| `workflow` | Print protocol documentation |

## Output Format

All commands output JSON to stdout:

```bash
$ bacchus claim TASK-1 agent-a
{
  "success": true,
  "bead_id": "TASK-1",
  "owner": "agent-a",
  "message": "Task claimed successfully"
}
```

## Supported Languages

Bacchus uses native tree-sitter for fast AST parsing:

- TypeScript / JavaScript
- Python
- Go
- Rust

## Agent Workflow

```
1. claim        - Take ownership of a task
2. workplan     - Declare what you'll modify (enables conflict detection)
3. heartbeat    - Keep alive every 5 min (check for notifications)
4. footprint    - Report changes when done
5. release      - Hand back the task
```

## Performance

- **Binary size**: ~6MB
- **Startup time**: ~5ms
- **Index 100 files**: ~200ms
- **Symbol query**: <1ms

## License

MIT
