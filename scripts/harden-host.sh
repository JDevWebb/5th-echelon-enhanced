#!/usr/bin/env bash
# Hardens the Linux machine a 5th Echelon server or coordinator runs on
# (Debian or Ubuntu). install-server.sh hardens what it installs (the
# services' sandboxes, Caddy, the firewall); this does the rest of the host.
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
#   - fail2ban for SSH, with longer bans for repeat offenders
#   - kernel settings: no redirects, logged martians, hidden kernel
#     pointers, hardened BPF, no core dumps
#   - turns off services a server doesn't need (ModemManager, udisks2, atd)
#
# Options:
#   --ssh-users "A B"     who may log in over SSH (default: the user running
#                         sudo, or root)
#   --ignore-ip IP        never ban this address in fail2ban (repeatable)
#   --reboot-time HH:MM   when unattended upgrades may reboot (default 04:30,
#                         the machine's time zone)
#   --no-ssh              leave SSH as it is
#   --no-updates          don't install updates now
#   --confirm-ssh         keep the SSH change made by the last run (cancels
#                         its undo)
#   -h, --help
set -euo pipefail
umask 022

ssh_users="" ignore_ips=() reboot_time="04:30" do_ssh=1 do_updates=1 confirm=0
SSH_DROPIN=/etc/ssh/sshd_config.d/01-5th-echelon-hardening.conf
REVERT_UNIT=fes-ssh-revert

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --ssh-users) ssh_users="${2:?}"; shift ;;
    --ignore-ip) ignore_ips+=("${2:?}"); shift ;;
    --reboot-time) reboot_time="${2:?}"; shift ;;
    --no-ssh) do_ssh=0 ;;
    --no-updates) do_updates=0 ;;
    --confirm-ssh) confirm=1 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done
[ "$(id -u)" -eq 0 ] || die "run as root (sudo bash $0)"
command -v apt-get >/dev/null || die "this script is for Debian and Ubuntu"

if [ "$confirm" -eq 1 ]; then
  systemctl stop "$REVERT_UNIT.timer" 2>/dev/null || true
  say "Kept the SSH settings (their undo is cancelled)"
  exit 0
fi

[[ "$reboot_time" =~ ^([01][0-9]|2[0-3]):[0-5][0-9]$ ]] || die "--reboot-time is HH:MM"
for ip in "${ignore_ips[@]}"; do
  [[ "$ip" =~ ^[0-9a-fA-F:.]+(/[0-9]{1,3})?$ ]] || die "--ignore-ip $ip isn't an address or range"
done
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
sysctl -q --system >/dev/null
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
say "fail2ban: on for SSH ($banaction), repeat offenders banned for longer"

# --- SSH ------------------------------------------------------------------
if [ "$do_ssh" -eq 1 ]; then
  root_login=no
  for u in $ssh_users; do [ "$u" = root ] && root_login=prohibit-password; done
  tmp="$(mktemp)"
  cat > "$tmp" <<SSHD
# Written by harden-host.sh. Read before the other files here: the first
# value of a setting wins.
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
    systemd-run --quiet --unit="$REVERT_UNIT" --on-active=10min /bin/sh -c "$revert; systemctl reload ssh || systemctl reload sshd" >/dev/null
    systemctl reload ssh 2>/dev/null || systemctl reload sshd
    say "SSH hardened (users: $ssh_users). Log in again from a NEW terminal now, then run:"
    say "  sudo bash $0 --confirm-ssh"
    say "Without that, the change undoes itself in 10 minutes."
  else
    say "SSH already hardened"
  fi
  rm -f "$tmp"
fi

if [ -f /var/run/reboot-required ]; then
  say "An update needs a reboot: sudo reboot (or wait for $reboot_time)"
fi
