#!/usr/bin/env bash
# Hardens the Linux machine a 5th Echelon server or coordinator runs on
# (Debian or Ubuntu). install-server.sh hardens what it installs (the
# services' sandboxes, Caddy, and rules in a firewall that's already on); this
# does the rest of the host. Neither turns a firewall on.
#
#   sudo bash harden-host.sh [options]
#
# What it does, each step safe to repeat:
#   - installs pending updates; unattended upgrades also reboot when an
#     update needs it, at a quiet hour (--reboot-time)
#   - SSH: keys only, only the given users, no forwarding except local
#     tunnels (the admin API's), modern algorithms, tight limits. The change
#     undoes itself after 10 minutes unless confirmed (--confirm-ssh, from a
#     new login), so a mistake can't lock you out
#   - fail2ban for SSH, with longer bans for repeat offenders (never the
#     address you're logged in from)
#   - kernel settings: no redirects, logged martians, hidden kernel
#     pointers, hardened BPF, no core dumps
#   - turns off services a server doesn't need (ModemManager, udisks2, atd)
#
# Options:
#   --ssh-users "A B"     who may log in over SSH (default: the user running
#                         sudo, or root)
#   --ignore-ip IP        never ban this address in fail2ban (repeatable; the
#                         address you're logged in from is added too)
#   --reboot-time HH:MM   when unattended upgrades may reboot (default 04:30,
#                         the machine's time zone)
#   --ssh-port N          move SSH to port N (fewer bots knocking). Port 22
#                         stays open too until --confirm-ssh, run from a
#                         login on the new port; kept on later runs
#   --no-ssh              leave SSH as it is
#   --no-updates          don't install updates now
#   --confirm-ssh         keep the SSH change made by the last run (cancels
#                         its undo), and close port 22 if SSH moved (only
#                         while a login on the new port is open)
#   -h, --help
set -euo pipefail
umask 022

ssh_users="" ignore_ips=() reboot_time="04:30" do_ssh=1 do_updates=1 confirm=0 ssh_port="" operator_ip=""
# The port SSH moved to (kept for later runs), and whether 22 still waits to close.
PORT_FILE=/etc/5th-echelon/ssh-port
PORT_PENDING=/etc/5th-echelon/ssh-port-pending
SSH_DROPIN=/etc/ssh/sshd_config.d/01-5th-echelon-hardening.conf
REVERT_UNIT=fes-ssh-revert

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --ssh-users) ssh_users="${2:?}"; shift ;;
    --ignore-ip) ignore_ips+=("${2:?}"); shift ;;
    --reboot-time) reboot_time="${2:?}"; shift ;;
    --no-ssh) do_ssh=0 ;;
    --no-updates) do_updates=0 ;;
    --confirm-ssh) confirm=1 ;;
    --ssh-port) ssh_port="${2:?}"; shift ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done
[ "$(id -u)" -eq 0 ] || die "run as root (sudo bash $0)"
command -v apt-get >/dev/null || die "this script is for Debian and Ubuntu"

# Applies sshd's settings: on systemd's socket (Ubuntu 22.10+), the ports
# come from sshd_config through a generator, so the socket restarts.
apply_sshd() {
  if systemctl is-active --quiet ssh.socket 2>/dev/null; then
    systemctl daemon-reload
    systemctl restart ssh.socket
    systemctl reload ssh 2>/dev/null || true
  else
    systemctl reload ssh 2>/dev/null || systemctl reload sshd
  fi
}

if [ "$confirm" -eq 1 ]; then
  systemctl stop "$REVERT_UNIT.timer" 2>/dev/null || true
  say "Kept the SSH settings (their undo is cancelled)"
  if [ -f "$PORT_PENDING" ]; then
    port="$(cat "$PORT_FILE")"
    [[ "$port" =~ ^[0-9]+$ ]] || die "$PORT_FILE doesn't hold a port"
    # Only while a login on the new port is open, so closing 22 can't lock
    # everyone out. Asked of the kernel: sudo drops SSH_CONNECTION.
    if ! ss -Hnt state established "( sport = :$port )" 2>/dev/null | grep -q .; then
      die "no SSH login on port $port is open: log in on it first (ssh -p $port ...), then run --confirm-ssh there; port 22 closes next"
    fi
    sed -i '/^Port 22$/d' "$SSH_DROPIN"
    sshd -t || die "sshd doesn't accept the settings without port 22; nothing changed"
    apply_sshd
    if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then
      ufw delete allow 22/tcp >/dev/null 2>&1 || true
      ufw delete allow OpenSSH >/dev/null 2>&1 || true
    fi
    if [ -f /etc/fail2ban/jail.d/5th-echelon.local ]; then
      sed -i "s/^port = .*/port = $port/" /etc/fail2ban/jail.d/5th-echelon.local
      systemctl restart fail2ban
    fi
    rm -f "$PORT_PENDING"
    say "SSH is on port $port only; port 22 is closed"
  fi
  exit 0
fi
if [ -z "$ssh_port" ] && [ -s "$PORT_FILE" ]; then ssh_port="$(cat "$PORT_FILE")"; fi
if [ -n "$ssh_port" ]; then
  [[ "$ssh_port" =~ ^[0-9]+$ ]] && [ "$ssh_port" -ge 1024 ] && [ "$ssh_port" -le 65535 ] || die "--ssh-port is 1024 to 65535"
  case "$ssh_port" in 8000|8080|8700|8701|50051|21126|21127|21128|21129|2019) die "port $ssh_port is the server's" ;; esac
  if ss -Hltn "sport = :$ssh_port" 2>/dev/null | grep -v sshd | grep -q .; then
    ss -Hltnp "sport = :$ssh_port" | grep -q '"sshd"\|systemd' || die "something else listens on port $ssh_port"
  fi
fi

[[ "$reboot_time" =~ ^([01][0-9]|2[0-3]):[0-5][0-9]$ ]] || die "--reboot-time is HH:MM"
for ip in "${ignore_ips[@]}"; do
  [[ "$ip" =~ ^[0-9a-fA-F:.]+(/[0-9]{1,3})?$ ]] || die "--ignore-ip $ip isn't an address or range"
done
# The address this is run from (sudo drops SSH_CONNECTION; `who -m` names
# the login's terminal and where it came from): fail2ban never bans it, so
# a few wrong keys can't lock out the operator.
from="$(who -m 2>/dev/null | sed -n 's/.*(\(.*\)).*/\1/p' | head -1 || true)"
[ -n "$from" ] || from="$(printf '%s' "${SSH_CONNECTION:-}" | awk '{print $1}')"
if [[ "$from" =~ ^[0-9a-fA-F:.]+$ ]] && [[ "$from" == *[.:]*[.:]* ]]; then
  case " ${ignore_ips[*]} " in *" $from "*) ;; *) ignore_ips+=("$from") ;; esac
  operator_ip="$from"
fi
if [ -z "$ssh_users" ]; then ssh_users="${SUDO_USER:-root}"; fi
for u in $ssh_users; do
  id "$u" >/dev/null 2>&1 || die "--ssh-users: there's no user $u"
  home="$(getent passwd "$u" | cut -d: -f6)"
  [ -s "$home/.ssh/authorized_keys" ] || die "$u has no SSH keys ($home/.ssh/authorized_keys): they couldn't log in"
done

export DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a
APT=(apt-get -y -q -o DPkg::Lock::Timeout=600 -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold)

# --- Updates -------------------------------------------------------------
if [ "$do_updates" -eq 1 ]; then
  say "Installing updates"
  "${APT[@]}" update >/dev/null
  "${APT[@]}" full-upgrade >/dev/null
  "${APT[@]}" autoremove --purge >/dev/null
fi
"${APT[@]}" install unattended-upgrades fail2ban >/dev/null
cat > /etc/apt/apt.conf.d/20auto-upgrades <<'APT'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
APT
cat > /etc/apt/apt.conf.d/52-5th-echelon-unattended <<APT
// Written by harden-host.sh: security updates take effect, kernels included.
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "$reboot_time";
Unattended-Upgrade::Remove-Unused-Dependencies "true";
Unattended-Upgrade::Remove-Unused-Kernel-Packages "true";
// Caddy from its own repository (install-server.sh adds it): its fixes too, not only the
// distribution's. Added to the distribution's own list.
Unattended-Upgrade::Origins-Pattern { "origin=cloudsmith/caddy/stable"; };
APT
systemctl enable --now unattended-upgrades >/dev/null 2>&1 || true
say "Unattended upgrades: on, rebooting at $reboot_time when an update needs it"

# --- Kernel ---------------------------------------------------------------
cat > /etc/sysctl.d/80-5th-echelon-hardening.conf <<'SYSCTL'
# Written by harden-host.sh.
# No ICMP redirects (this machine isn't a router), and spoofed sources dropped.
net.ipv4.conf.all.send_redirects = 0
net.ipv4.conf.default.send_redirects = 0
net.ipv4.conf.all.accept_redirects = 0
net.ipv4.conf.default.accept_redirects = 0
net.ipv4.conf.all.secure_redirects = 0
net.ipv4.conf.default.secure_redirects = 0
net.ipv6.conf.all.accept_redirects = 0
net.ipv6.conf.default.accept_redirects = 0
net.ipv4.conf.all.accept_source_route = 0
net.ipv4.conf.default.accept_source_route = 0
net.ipv6.conf.all.accept_source_route = 0
net.ipv4.conf.all.rp_filter = 1
net.ipv4.conf.default.rp_filter = 1
net.ipv4.conf.all.log_martians = 1
net.ipv4.conf.default.log_martians = 1
net.ipv4.icmp_echo_ignore_broadcasts = 1
net.ipv4.icmp_ignore_bogus_error_responses = 1
net.ipv4.tcp_syncookies = 1
net.ipv4.tcp_rfc1337 = 1
# Kernel addresses and logs hidden from users; BPF hardened.
kernel.kptr_restrict = 2
kernel.dmesg_restrict = 1
kernel.unprivileged_bpf_disabled = 1
net.core.bpf_jit_harden = 2
kernel.perf_event_paranoid = 3
kernel.yama.ptrace_scope = 2
kernel.sysrq = 0
kernel.randomize_va_space = 2
# No core dumps of setuid programs; protected links and FIFOs.
fs.suid_dumpable = 0
fs.protected_symlinks = 1
fs.protected_hardlinks = 1
fs.protected_fifos = 2
fs.protected_regular = 2
SYSCTL
# Only these settings, and past any this kernel doesn't have (a container,
# an older kernel): the rest still apply.
if ! sysctl -q -e -p /etc/sysctl.d/80-5th-echelon-hardening.conf >/dev/null 2>&1; then
  warn "some kernel settings couldn't be set here (a container?); the others are, and all apply at the next boot"
fi
install -d -m 755 /etc/systemd/coredump.conf.d
printf '[Coredump]\nStorage=none\nProcessSizeMax=0\n' > /etc/systemd/coredump.conf.d/50-5th-echelon.conf
printf '* hard core 0\n' > /etc/security/limits.d/50-5th-echelon-no-core.conf
say "Kernel settings hardened; no core dumps"

# --- Services a server doesn't need -----------------------------------------
for s in ModemManager udisks2 atd; do
  if systemctl list-unit-files "$s.service" >/dev/null 2>&1 && systemctl cat "$s.service" >/dev/null 2>&1; then
    systemctl disable --now "$s.service" >/dev/null 2>&1 || true
    systemctl mask "$s.service" >/dev/null 2>&1 || true
    say "Turned off $s"
  fi
done

# --- fail2ban -------------------------------------------------------------
# Moving SSH: port 22 stays open alongside until --confirm-ssh.
transition=0
if [ -n "$ssh_port" ] && { [ -f "$PORT_PENDING" ] || [ "$(cat "$PORT_FILE" 2>/dev/null)" != "$ssh_port" ]; }; then transition=1; fi
f2b_port=ssh
if [ -n "$ssh_port" ]; then
  if [ "$transition" -eq 1 ]; then f2b_port="22,$ssh_port"; else f2b_port="$ssh_port"; fi
fi
banaction=nftables-multiport
if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then banaction=ufw; fi
cat > /etc/fail2ban/jail.d/5th-echelon.local <<JAIL
# Written by harden-host.sh.
[DEFAULT]
backend = systemd
banaction = $banaction
ignoreip = 127.0.0.1/8 ::1 ${ignore_ips[*]}
findtime = 10m
maxretry = 5
bantime = 1h
# Each ban of the same address lasts longer, up to a week.
bantime.increment = true
bantime.maxtime = 1w

[sshd]
enabled = true
port = $f2b_port
mode = aggressive

[recidive]
enabled = true
logpath = /var/log/fail2ban.log
banaction = $banaction
bantime = 1w
findtime = 1d
JAIL
systemctl enable fail2ban >/dev/null 2>&1
systemctl restart fail2ban
say "fail2ban: on for SSH ($banaction), repeat offenders banned for longer${operator_ip:+; never $operator_ip (this login)}"

# --- SSH ------------------------------------------------------------------
if [ "$do_ssh" -eq 1 ]; then
  ports=""
  if [ -n "$ssh_port" ]; then
    if [ "$transition" -eq 1 ]; then ports="Port 22"$'\n'"Port $ssh_port"; else ports="Port $ssh_port"; fi
  fi
  root_login=no
  for u in $ssh_users; do [ "$u" = root ] && root_login=prohibit-password; done
  tmp="$(mktemp)"
  cat > "$tmp" <<SSHD
# Written by harden-host.sh. Read before the other files here: the first
# value of a setting wins.
$ports
PermitRootLogin $root_login
AllowUsers $ssh_users
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitEmptyPasswords no
AuthenticationMethods publickey
# Local tunnels only (the admin API's); nothing else forwarded.
AllowTcpForwarding local
AllowAgentForwarding no
AllowStreamLocalForwarding no
X11Forwarding no
PermitTunnel no
GatewayPorts no
PermitUserEnvironment no
MaxAuthTries 3
MaxSessions 4
MaxStartups 10:30:60
LoginGraceTime 20
ClientAliveInterval 300
ClientAliveCountMax 2
KexAlgorithms mlkem768x25519-sha256,sntrup761x25519-sha512,sntrup761x25519-sha512@openssh.com,curve25519-sha256,curve25519-sha256@libssh.org
Ciphers chacha20-poly1305@openssh.com,aes256-gcm@openssh.com,aes128-gcm@openssh.com,aes256-ctr,aes128-ctr
MACs hmac-sha2-512-etm@openssh.com,hmac-sha2-256-etm@openssh.com,umac-128-etm@openssh.com
SSHD
  # Algorithms this OpenSSH doesn't know (an older release) are left out.
  if ! sshd -t -f /etc/ssh/sshd_config -o "KexAlgorithms=$(sed -n 's/^KexAlgorithms //p' "$tmp")" 2>/dev/null; then
    sed -i 's/^KexAlgorithms .*/KexAlgorithms curve25519-sha256,curve25519-sha256@libssh.org/' "$tmp"
  fi
  old="$(cat "$SSH_DROPIN" 2>/dev/null || true)"
  if [ "$old" != "$(cat "$tmp")" ]; then
    install -m 644 "$tmp" "$SSH_DROPIN"
    if ! sshd -t; then
      if [ -n "$old" ]; then printf '%s\n' "$old" > "$SSH_DROPIN"; else rm -f "$SSH_DROPIN"; fi
      rm -f "$tmp"
      die "sshd doesn't accept the new settings; nothing changed"
    fi
    # Undone in 10 minutes unless confirmed from a new login.
    systemctl stop "$REVERT_UNIT.timer" "$REVERT_UNIT.service" 2>/dev/null || true
    systemctl reset-failed "$REVERT_UNIT.timer" "$REVERT_UNIT.service" 2>/dev/null || true
    if [ -n "$old" ]; then
      printf '%s\n' "$old" > /root/.5th-echelon-ssh-previous
      revert="cp /root/.5th-echelon-ssh-previous $SSH_DROPIN"
    else
      revert="rm -f $SSH_DROPIN"
    fi
    if [ -n "$ssh_port" ]; then
      install -d -m 755 "$(dirname "$PORT_FILE")"
      [ "$transition" -eq 0 ] || revert="$revert; rm -f $PORT_PENDING $( [ "$(cat "$PORT_FILE" 2>/dev/null)" = "$ssh_port" ] || echo "$PORT_FILE")"
      echo "$ssh_port" > "$PORT_FILE"
      [ "$transition" -eq 0 ] || touch "$PORT_PENDING"
      if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then ufw allow "$ssh_port/tcp" >/dev/null; fi
    fi
    systemd-run --quiet --unit="$REVERT_UNIT" --on-active=10min /bin/sh -c "$revert; systemctl daemon-reload; systemctl restart ssh.socket 2>/dev/null; systemctl reload ssh 2>/dev/null || systemctl reload sshd" >/dev/null
    apply_sshd
    say "SSH hardened (users: $ssh_users${ssh_port:+, port $ssh_port}). Log in again from a NEW terminal${ssh_port:+ on port $ssh_port (ssh -p $ssh_port)} now, then run:"
    say "  sudo bash $0 --confirm-ssh"
    say "Without that, the change undoes itself in 10 minutes."
  else
    say "SSH already hardened"
  fi
  rm -f "$tmp"
  # What sshd uses now: a file read before this one (an earlier name in
  # sshd_config.d, or sshd_config above its Include) wins.
  if effective="$(sshd -T 2>/dev/null)"; then
    unmet=()
    wants=("passwordauthentication no" "kbdinteractiveauthentication no" "pubkeyauthentication yes" "maxauthtries 3"
      "allowtcpforwarding local" "allowagentforwarding no" "x11forwarding no" "permittunnel no")
    [ "$root_login" = no ] && wants+=("permitrootlogin no")
    for u in $ssh_users; do wants+=("allowusers $u"); done
    if [ -n "$ssh_port" ]; then wants+=("port $ssh_port"); fi
    for want in "${wants[@]}"; do
      printf '%s\n' "$effective" | grep -qix "$want" || unmet+=("$want")
    done
    if [ "${#unmet[@]}" -gt 0 ]; then
      warn "sshd doesn't use these settings, so another file sets them first: ${unmet[*]}. Look in /etc/ssh/sshd_config and /etc/ssh/sshd_config.d/ (files before $(basename "$SSH_DROPIN"))"
    else
      say "sshd uses the settings (sshd -T)"
    fi
  else
    warn "couldn't read sshd's settings back (sshd -T)"
  fi
fi

if [ -f /var/run/reboot-required ]; then
  say "An update needs a reboot: sudo reboot (or wait for $reboot_time)"
fi
