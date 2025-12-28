#!/bin/bash
# Bacchus Stop Hook
# Delegates to `bacchus session check` for all decision logic
#
# Exit codes:
#   0 + JSON with decision  = structured response

# Check if bacchus is available
if ! command -v bacchus &> /dev/null; then
    echo '{"decision": "approve", "reason": "bacchus not installed"}'
    exit 0
fi

# Consume stdin (required by hook protocol)
cat > /dev/null

# Delegate to bacchus session check (handles all logic in Rust with serde)
bacchus session check
