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
# Capture output and exit code; fail-open if bacchus errors
output=$(bacchus session check 2>&1)
exit_code=$?

if [ $exit_code -ne 0 ]; then
    # bacchus failed - approve exit (fail-open) but include error info
    echo "{\"decision\": \"approve\", \"reason\": \"bacchus error: ${output//\"/\\\"}\"}"
    exit 0
fi

# Output the result from bacchus
echo "$output"
