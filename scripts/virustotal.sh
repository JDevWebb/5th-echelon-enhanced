#!/usr/bin/env bash
# Scans files with VirusTotal and prints a Markdown table of the results,
# with a link to each file's live report. The release workflow puts it in
# the release notes; run it by hand to refresh them.
#
#   VT_API_KEY=... scripts/virustotal.sh dist/launcher.exe dist/uplay_r1_loader.dll ...
#
# A free VirusTotal account's key is enough: it allows 4 requests a minute,
# so this waits between them. A file VirusTotal already knows isn't uploaded
# again; its existing report is used.
set -euo pipefail

: "${VT_API_KEY:?set VT_API_KEY to a VirusTotal API key}"
[ $# -gt 0 ] || { echo "usage: $0 <file>..." >&2; exit 2; }
command -v jq >/dev/null || { echo "needs jq" >&2; exit 2; }

API=https://www.virustotal.com/api/v3
# Free keys: 4 requests a minute.
pace() { sleep 16; }
vt() { curl -fsS --proto '=https' --retry 3 -H "x-apikey: $VT_API_KEY" "$@"; }

# Waits for an analysis and prints "malicious suspicious total".
wait_for() {
  local id="$1" out status
  for _ in $(seq 40); do
    pace
    out="$(vt "$API/analyses/$id")"
    status="$(jq -r '.data.attributes.status' <<<"$out")"
    if [ "$status" = completed ]; then
      jq -r '.data.attributes.stats | "\(.malicious) \(.suspicious) \([.[]] | add)"' <<<"$out"
      return 0
    fi
  done
  echo "timeout"
}

# Prints "malicious suspicious total" from a known file's last analysis, or nothing.
known() {
  local out
  out="$(vt "$API/files/$1" 2>/dev/null)" || return 0
  jq -r '.data.attributes.last_analysis_stats | "\(.malicious) \(.suspicious) \([.[]] | add)"' <<<"$out"
}

echo "| File | VirusTotal | SHA-256 |"
echo "|---|---|---|"
for f in "$@"; do
  name="$(basename "$f")"
  sha="$(sha256sum "$f" | cut -d' ' -f1)"
  result="$(known "$sha")"
  pace
  if [ -z "$result" ]; then
    size="$(stat -c %s "$f" 2>/dev/null || stat -f %z "$f")"
    if [ "$size" -gt 33554432 ]; then
      # Over 32 MB: VirusTotal hands out a one-off upload address.
      url="$(vt "$API/files/upload_url" | jq -r '.data')"
      pace
    else
      url="$API/files"
    fi
    id="$(vt -X POST -F "file=@$f" "$url" | jq -r '.data.id')"
    result="$(wait_for "$id")"
  fi
  link="https://www.virustotal.com/gui/file/$sha"
  if [ "$result" = timeout ] || [ -z "$result" ]; then
    verdict="[scan pending]($link)"
  else
    read -r malicious suspicious total <<<"$result"
    flagged=$((malicious + suspicious))
    verdict="[**$flagged / $total** engines]($link)"
  fi
  echo "| \`$name\` | $verdict | \`$sha\` |"
done
