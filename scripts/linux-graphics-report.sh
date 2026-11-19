#!/usr/bin/env bash
# A report of what draws the game on a Linux PC (Steam, Proton), for a graphics problem: the
# system, the GPU and its driver, the Proton Steam uses for Splinter Cell: Blacklist and its
# launch options, files in the game folder that change how it draws (DXVK, ReShade, .asi, configs),
# and the drawing lines of a Proton log if there is one. It only reads; it writes one file,
# ~/5th-echelon-graphics-report.txt, to send to the 5th Echelon admins. Your home folder, user
# name and computer name are replaced in it.
#
# For the most useful report, first:
#   1. Steam > Splinter Cell: Blacklist > Properties > Launch options:
#        PROTON_LOG=1 DXVK_LOG_LEVEL=info %command%
#   2. Play until the problem shows (e.g. the spy's goggles), then quit the game.
#   3. Run this:  bash linux-graphics-report.sh
#   4. Send ~/5th-echelon-graphics-report.txt, and take the launch options out again (the log
#      grows every time the game runs).
set -u

APP=235600
OUT="$HOME/5th-echelon-graphics-report.txt"
HOST=$(hostname 2>/dev/null || cat /etc/hostname 2>/dev/null || echo "")

# Home folder, user name and computer name out of everything written.
mask() {
  local h=${HOME//\//\\/}
  sed -e "s/$h/~/g" \
      -e "s/\b${USER:-nobody-at-all}\b/<user>/g" \
      ${HOST:+-e "s/\b$HOST\b/<computer>/g"}
}

section() { printf '\n== %s\n' "$1"; }
have() { command -v "$1" >/dev/null 2>&1; }

# Steam's folders: the usual one, the older link, the Flatpak's.
steam_roots() {
  local d
  for d in "$HOME/.local/share/Steam" "$HOME/.steam/steam" "$HOME/.steam/root" \
           "$HOME/.var/app/com.valvesoftware.Steam/.local/share/Steam"; do
    [ -d "$d/steamapps" ] && readlink -f "$d"
  done | sort -u
}

# The libraries Steam knows (libraryfolders.vdf "path" lines), each with steamapps in it.
libraries() {
  local root=$1
  echo "$root"
  sed -n 's/^[[:space:]]*"path"[[:space:]]*"\(.*\)"/\1/p' "$root/steamapps/libraryfolders.vdf" 2>/dev/null
}

# The block for key "$2" under the block "$1" of a VDF file, as text (good enough to read).
vdf_block() {
  awk -v outer="\"$2\"" -v section="\"$3\"" '
    $1 == section { in_section = 1 }
    in_section && $1 == outer { grab = 1; depth = 0 }
    grab { print; if ($0 ~ /{/) depth++; if ($0 ~ /}/) { depth--; if (depth == 0) exit } }
  ' "$1" 2>/dev/null
}

{
  echo "5th Echelon graphics report, $(date -u +%Y-%m-%dT%H:%MZ)"

  section "System"
  . /etc/os-release 2>/dev/null && echo "OS: ${PRETTY_NAME:-?}"
  echo "Kernel: $(uname -r) ($(uname -m))"
  echo "Session: ${XDG_SESSION_TYPE:-?}, desktop: ${XDG_CURRENT_DESKTOP:-?}${WAYLAND_DISPLAY:+, Wayland display set}${DISPLAY:+, X display set}"
  have gamescope && echo "gamescope: installed ($(gamescope --version 2>&1 | head -1))"

  section "GPU (lspci)"
  if have lspci; then
    lspci -nnk 2>/dev/null | grep -A3 -E 'VGA|3D|Display' | grep -v -E 'Subsystem'
  else
    echo "lspci not found (pciutils)"
  fi

  [ -r /proc/driver/nvidia/version ] && head -1 /proc/driver/nvidia/version | sed 's/^/NVIDIA driver: /'
  # The 32-bit ones (i686) matter: the game is 32-bit.
  icds=""
  for f in /usr/share/vulkan/icd.d/*.json /etc/vulkan/icd.d/*.json; do
    [ -e "$f" ] && icds="$icds ${f##*/}"
  done
  echo "Vulkan drivers installed:${icds:- none found}"

  section "Vulkan (vulkaninfo --summary)"
  if have vulkaninfo; then
    vulkaninfo --summary 2>/dev/null | grep -E 'Vulkan Instance Version|GPU[0-9]|deviceName|deviceType|driverID|driverName|driverInfo|apiVersion|conformanceVersion'
  else
    echo "vulkaninfo not found (vulkan-tools)"
  fi

  section "OpenGL (glxinfo -B)"
  if have glxinfo; then
    glxinfo -B 2>/dev/null | grep -E 'OpenGL vendor|OpenGL renderer|OpenGL core profile version|Version'
  else
    echo "glxinfo not found (mesa-utils)"
  fi

  section "Steam"
  roots=$(steam_roots)
  [ -n "$roots" ] || echo "No Steam folder found"
  while IFS= read -r root; do
    [ -n "$root" ] || continue
    echo "Steam folder: $root"
    echo "-- Proton chosen (config.vdf CompatToolMapping: $APP for this game, 0 for the default)"
    for key in "$APP" 0; do
      vdf_block "$root/config/config.vdf" "$key" CompatToolMapping | grep -E '"name"|"config"' | sed "s/^[[:space:]]*/   $key: /"
    done
    echo "-- Launch options"
    for f in "$root"/userdata/*/config/localconfig.vdf; do
      [ -f "$f" ] || continue
      awk -v app="\"$APP\"" '
        $1 == app { grab = 1; depth = 0 }
        grab {
          if ($1 == "\"LaunchOptions\"") { sub(/^[[:space:]]*/, "   "); print; exit }
          if ($0 ~ /{/) depth++
          if ($0 ~ /}/) { depth--; if (depth == 0) exit }
        }' "$f"
    done
    libs=$(libraries "$root")
    echo "-- Proton versions installed"
    while IFS= read -r lib; do
      ls -d "$lib"/steamapps/common/Proton* 2>/dev/null | sed 's/^/   /'
    done <<<"$libs"
    ls -d "$root"/compatibilitytools.d/* 2>/dev/null | sed 's/^/   (custom) /'
    while IFS= read -r lib; do
      pfx="$lib/steamapps/compatdata/$APP"
      [ -d "$pfx" ] && echo "-- The game's Proton prefix: $pfx (made by $(cat "$pfx/version" 2>/dev/null || echo '?'))"
    done <<<"$libs"
    while IFS= read -r lib; do
      game="$lib/steamapps/common/Tom Clancy's Splinter Cell Blacklist"
      [ -d "$game" ] || continue
      echo "-- The game: $game"
      sys="$game/src/SYSTEM"
      echo "   Files that change how it draws or loads (DLLs Windows has too, mods, configs):"
      find "$sys" -maxdepth 1 -type f \( -iname 'd3d*.dll' -o -iname 'dxgi.dll' -o -iname 'dinput8.dll' \
          -o -iname 'version.dll' -o -iname 'winmm.dll' -o -iname '*.asi' -o -iname 'dxvk.conf' \
          -o -iname '*reshade*' -o -iname '*.ini' \) -printf '     %f (%s bytes, %TY-%Tm-%Td)\n' 2>/dev/null | sort
      for d in scripts plugins reshade-shaders; do
        [ -d "$sys/$d" ] && echo "     folder: $d"
      done
      grep -a -m1 -E 'add-on release' "$sys/bl-tracing.log" 2>/dev/null | sed 's/^.*hooks: [0-9]*: /   Last game: /'
      grep -a -m1 -E 'Detected DX version' "$sys/bl-tracing.log" 2>/dev/null | sed 's/^.*Detected DX version /   Last game drew with: /'
    done <<<"$libs"
  done <<<"$roots"

  section "Environment (drawing-related variables of this shell)"
  vars=$(env | grep -E '^(PROTON|DXVK|VKD3D|WINE|MESA|RADV|AMD_|__GL|__NV|VK_|ENABLE_|DRI_|LIBGL|GAMESCOPE|SDL_VIDEO)' | sort)
  echo "${vars:-(none)}"

  section "Proton log (PROTON_LOG=1: ~/steam-$APP.log)"
  log="$HOME/steam-$APP.log"
  if [ -f "$log" ]; then
    echo "Written $(date -u -r "$log" +%Y-%m-%dT%H:%MZ), $(wc -l <"$log") lines; the drawing lines:"
    grep -a -E 'Proton:|^Options|DXVK: v|Device properties|Device name|Driver *:|Driver version|Vulkan:|Using feature level|D3D11CreateDevice|Presenter|Swap ?chain|wine: |err:(d3d|vulkan|dxgi|ntdll)|warn:(d3d|vulkan|dxgi)|DXVK.*(fail|error)' "$log" \
      | awk '!seen[$0]++' | head -150
  else
    echo "None: add the launch options at the top of this script, play until the problem shows, quit, and run this again."
  fi
} 2>&1 | mask >"$OUT"

echo "Written: $OUT"
echo "Please send that file to the 5th Echelon admins (Support in the launcher, or Discord)."
