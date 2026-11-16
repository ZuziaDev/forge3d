#!/usr/bin/env bash
# Print the GitHub release body for a tag: logo, highlights from the matching
# CHANGELOG.md section, install steps, Claude Desktop config and SHA256 sums.
# Usage: packaging/release-notes.sh vX.Y.Z [dist-dir]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
tag="${1:?usage: release-notes.sh vX.Y.Z [dist-dir]}"
dist="${2:-$root/dist}"
version="${tag#v}"
repo="${GITHUB_REPOSITORY:-ZuziaDev/forge3d}"

# "## [0.1.1] - date" ... up to the next "## " heading.
section="$(awk -v v="$version" '
  /^## \[/ { if (on) exit; if (index($0, "[" v "]")) { on = 1; next } }
  on { print }
' "$root/CHANGELOG.md" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}')"
if [ -z "$section" ]; then
  echo "CHANGELOG.md has no section for $version" >&2
  exit 1
fi

sums=""
if [ -f "$dist/SHA256SUMS.txt" ]; then
  sums="$(cat "$dist/SHA256SUMS.txt")"
fi

cat <<NOTES
<p align="center"><img src="https://raw.githubusercontent.com/$repo/$tag/assets/logo.png" width="112" alt="forge3d logo"></p>

# forge3d $version

**forge3d by ZuziaDev**: a Blender-style 3D modeler written in Rust with a built-in MCP server.
You and your AI assistant edit the same scene, and every change lands in one undo history.

## What's new

$section

## Install (Windows)

1. Download **forge3d-$version-setup.exe** below and run it. It installs for the current user and needs no administrator rights.
2. Start **forge3d** from the Start menu or the desktop shortcut.
3. No installer? Use **forge3d-$version-windows-x64.exe**: run \`forge3d-$version-windows-x64.exe gui\`.

The builds are cross-compiled on Linux and not code-signed, so Windows SmartScreen may ask for confirmation ("More info", then "Run anyway").

## Connect Claude Desktop

Add this to \`claude_desktop_config.json\` (the installer writes a ready-made copy with your real paths to the install folder):

\`\`\`json
{
  "mcpServers": {
    "forge3d": {
      "command": "C:\\\\Users\\\\YOU\\\\AppData\\\\Local\\\\Programs\\\\forge3d\\\\forge3d.exe",
      "args": ["mcp", "--workspace", "C:\\\\Users\\\\YOU\\\\Documents\\\\forge3d"]
    }
  }
}
\`\`\`

To drive a running window instead: \`claude mcp add --transport http forge3d http://127.0.0.1:7878/mcp\`.

## Checksums (SHA256)

\`\`\`
$sums
\`\`\`

Full history: [CHANGELOG.md](https://github.com/$repo/blob/$tag/CHANGELOG.md)
NOTES
