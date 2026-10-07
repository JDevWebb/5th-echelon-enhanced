#!/usr/bin/env bash
# What install-server.sh writes and runs as root on a server, end to end in a throwaway
# container, from the installer's own functions:
#
# * the updater: a signed release installs and comes back healthy; one that doesn't is
#   rolled back, with the databases as they were; a bad signature, an older release and
#   a retry too soon are refused; a standby's coordinator is updated but not started;
# * backup.sh: the live copy (Litestream) and the archive, restoring from either, as at a
#   time, and from another machine with the coordinator's files (docs/backups.md);
# * failover: a standby takes over a coordinator that stays down, from its live backup;
#   the one replaced stands down when it's back; --take-over moves it back
#   (docs/failover.md).
#
# GitHub, Cloudflare's API and R2/B2 are stand-ins in the container (a test CA, a mock
# API, rclone's S3 server). Litestream and rclone are downloaded at the installer's
# pinned versions and checked against its checksums.
#
#   scripts/ops-test.sh <folder with a coordinator built for the container>
#
# IMAGE is the container (default fes-build:local; it needs apt, or bash, curl, python3,
# openssl, unzip and ca-certificates); BIN_MOUNT as in federation-test.sh. Used by
# `build/build.sh ops-test` and CI.
# shellcheck disable=SC2034 # the installer's functions (eval'd) read the variables set here
set -euo pipefail

if [ "${1:-}" != --inside ]; then
  bin=${1:?folder with the coordinator, inside the container}
  image=${IMAGE:-fes-build:local}
  mount=${BIN_MOUNT:--v fes-target:/target:ro}
  here=$(cd "$(dirname "$0")/.." && pwd)
  # shellcheck disable=SC2086
  exec docker run --rm $mount -v "$here/scripts:/ops:ro" "$image" timeout 1200 bash /ops/ops-test.sh --inside "$bin"
fi

bin="$2"
S=/ops/install-server.sh
rc=0
pass() { echo "PASS $*"; }
fail() { echo "FAIL $*"; rc=1; }
check() { # check "what" command...
  local what="$1"; shift
  if "$@"; then pass "$what"; else fail "$what"; fi
}
# A function of install-server.sh, as it is there: up to its closing brace, skipping the
# here-documents in it (whose lines can look like a function's end).
fn() {
  awk -v f="$1" '
    !on && $0 ~ "^" f "\\(\\) \\{$" { on = 1 }
    !on { next }
    { print }
    delim != "" { if ($0 == delim) delim = ""; next }
    match($0, /<<-?'"'"'?[A-Z_]+'"'"'?/) { d = substr($0, RSTART, RLENGTH); gsub(/[<'"'"'-]/, "", d); delim = d; next }
    $0 == "}" { exit }' "$S"
}
say() { echo "  ==> $*"; }
warn() { echo "  warning: $*"; }
die() { echo "  error: $*"; exit 1; }

# --- Tools ------------------------------------------------------------------

need=()
for c in curl python3 openssl unzip update-ca-certificates setsid; do command -v "$c" >/dev/null || need+=("$c"); done
if [ "${#need[@]}" -gt 0 ]; then
  apt-get -qq update >/dev/null && apt-get -qq install -y --no-install-recommends curl python3 openssl unzip ca-certificates util-linux >/dev/null 2>&1
fi
cd /tmp
arch=amd64; [ "$(uname -m)" = aarch64 ] && arch=arm64
eval "$(grep -E '^(LITESTREAM|RCLONE)_(VERSION|SHA256_(amd64|arm64))=' "$S")"
ls_sum="LITESTREAM_SHA256_$arch" rc_sum="RCLONE_SHA256_$arch"
curl -fsSL -o ls.tgz "https://github.com/benbjohnson/litestream/releases/download/v$LITESTREAM_VERSION/litestream-$LITESTREAM_VERSION-linux-$( [ $arch = arm64 ] && echo arm64 || echo x86_64 ).tar.gz"
echo "${!ls_sum}  ls.tgz" | sha256sum -c --quiet - && tar -xzf ls.tgz litestream && install litestream /usr/local/bin/
curl -fsSL -o rc.zip "https://github.com/rclone/rclone/releases/download/v$RCLONE_VERSION/rclone-v$RCLONE_VERSION-linux-$arch.zip"
echo "${!rc_sum}  rc.zip" | sha256sum -c --quiet - && unzip -q -j rc.zip "rclone-v$RCLONE_VERSION-linux-$arch/rclone" && install rclone /usr/local/bin/
install -m 755 "$bin/coordinator" /usr/local/bin/fes-coordinator

# The installer's settings, as it has them.
eval "$(sed -n '/^REPO=/,/^CADDY_SHA512_arm64=/p' "$S")"
mkdir -p /etc/systemd/system "$ETC_DIR" "$STATE_DIR" "$PROGRAM_DIR" "$UPDATE_DIR"

# systemctl, for the services the tests run: a service is a program started in the
# background (its pid in /tmp/svc-NAME.pid). Every call is logged.
cat > /usr/local/bin/systemctl <<'SH'
#!/bin/bash
echo "systemctl $*" >> /tmp/systemctl.log
quiet=0; [ "${2:-}" = --quiet ] && quiet=1
unit="${*: -1}"; unit="${unit%.service}"
alive() { [ -f "/tmp/svc-$1.pid" ] && kill -0 "$(cat "/tmp/svc-$1.pid")" 2>/dev/null; }
case "$1" in
  is-active) if alive "$unit"; then [ $quiet = 1 ] || echo active; exit 0; fi; [ $quiet = 1 ] || echo inactive; exit 3 ;;
  is-enabled) [ -f "/tmp/enabled-$unit" ] ;;
  start|restart)
    [ "$1" = restart ] && /tmp/svc.sh stop "$unit"
    /tmp/svc.sh start "$unit" ;;
  stop) /tmp/svc.sh stop "$unit" ;;
  *) exit 0 ;;
esac
SH
chmod 755 /usr/local/bin/systemctl
cat > /tmp/svc.sh <<'SH'
#!/bin/bash
# svc.sh start|stop NAME: the services' commands are in /tmp/svc-NAME.cmd. Like systemd, a
# service gets none of the caller's open files (the updater's lock, say).
for fd in /proc/$$/fd/*; do fd=${fd##*/}; [ "$fd" -gt 2 ] && eval "exec $fd>&-"; done 2>/dev/null
alive() { [ -f "/tmp/svc-$1.pid" ] && kill -0 "$(cat "/tmp/svc-$1.pid")" 2>/dev/null; }
case "$1" in
  start)
    alive "$2" && exit 0
    [ -f "/tmp/svc-$2.cmd" ] || exit 0
    setsid bash "/tmp/svc-$2.cmd" >> "/tmp/svc-$2.log" 2>&1 < /dev/null & echo $! > "/tmp/svc-$2.pid"
    sleep 0.5 ;;
  stop)
    if alive "$2"; then kill -- -"$(cat "/tmp/svc-$2.pid")" 2>/dev/null || kill "$(cat "/tmp/svc-$2.pid")"; fi
    rm -f "/tmp/svc-$2.pid"; sleep 0.3 ;;
esac
SH
chmod 755 /tmp/svc.sh

# --- The updater --------------------------------------------------------------

echo "--- the updater"
# GitHub: a test CA, a certificate for github.com, and a release server on this machine.
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 -subj /CN=test-ca -keyout ca.key -out ca.crt 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=github.com -keyout gh.key -out gh.csr 2>/dev/null
printf 'subjectAltName=DNS:github.com\n' > gh.ext
openssl x509 -req -in gh.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 -extfile gh.ext -out gh.crt 2>/dev/null
cp ca.crt /usr/local/share/ca-certificates/fes-test-ca.crt && update-ca-certificates >/dev/null 2>&1
echo "127.0.0.1 github.com" >> /etc/hosts
mkdir -p /tmp/www
python3 - <<'PY' &
import http.server, ssl, functools
h = functools.partial(http.server.SimpleHTTPRequestHandler, directory="/tmp/www")
h.log_message = lambda *a: None
s = http.server.ThreadingHTTPServer(("127.0.0.1", 443), h)
c = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); c.load_cert_chain("/tmp/gh.crt", "/tmp/gh.key")
s.socket = c.wrap_socket(s.socket, server_side=True)
s.serve_forever()
PY
# The release key: a test one, which install-server.sh carries in its place here.
openssl genpkey -algorithm ed25519 -out release.key 2>/dev/null
openssl genpkey -algorithm ed25519 -out other.key 2>/dev/null
REPO=test/fes
RELEASE_KEY_PEM="$(openssl pkey -in release.key -pubout)"
# The programs: a server answering /api/info (or the coordinator /v1/info) with its
# release; a broken one changes the database and answers as nothing.
cat > /tmp/fake.py <<'PY'
import http.server, json, sys
version, mode, addr, path = sys.argv[1:5]
if mode == "broken":
    open("/var/lib/5th-echelon/5th-echelon.db", "w").write("migrated by a broken release\n")
    version = "broken"
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        b = json.dumps({"version": version}, separators=(",", ":")).encode()
        self.send_response(200 if self.path == path else 404); self.end_headers(); self.wfile.write(b)
http.server.HTTPServer((addr, int(sys.argv[5])), H).serve_forever()
PY
program() { # program VERSION MODE server|coordinator
  case "$3" in
    server) printf '#!/bin/bash\nexec python3 /tmp/fake.py %s %s 127.0.0.1 /api/info 80\n' "$1" "$2" ;;
    coordinator) printf '#!/bin/bash\nexec python3 /tmp/fake.py %s %s %s /v1/info 8700\n' "$1" "$2" "$COORD_ADDR" ;;
  esac
}
release() { # release VERSION MODE [KEY]
  local d="/tmp/www/$REPO/releases/download/v$1"
  mkdir -p "$d"
  program "$1" "$2" server > "$d/dedicated_server-linux-x86_64"
  program "$1" "$2" coordinator > "$d/coordinator-linux-x86_64"
  (cd "$d" && sha256sum dedicated_server-linux-x86_64 coordinator-linux-x86_64 > SHA256SUMS)
  { printf '5th-echelon/release/v2\n%s\n' "$1"; cat "$d/SHA256SUMS"; } > /tmp/signed
  openssl pkeyutl -sign -inkey "${3:-release.key}" -rawin -in /tmp/signed -out /tmp/sig
  base32 -w0 /tmp/sig | tr -d '=' > "$d/SHA256SUMS.sig"
}
release 0.9.1 good
release 0.9.2 broken
release 0.9.3 good other.key
# What runs now: 0.9.0, the server and a standby's coordinator (installed, not running).
program 0.9.0 good server > "$PROGRAM_DIR/dedicated_server"
program 0.9.0 good coordinator > "$PROGRAM_DIR/coordinator"
chmod 755 "$PROGRAM_DIR/dedicated_server" "$PROGRAM_DIR/coordinator"
echo "exec $PROGRAM_DIR/dedicated_server" > /tmp/svc-5th-echelon.cmd
echo "exec $PROGRAM_DIR/coordinator" > /tmp/svc-5th-echelon-coordinator.cmd
echo "exec sleep 1000" > /tmp/svc-5th-echelon-standby.cmd
touch /tmp/enabled-5th-echelon-standby
mkdir -p "$COORD_DIR"
echo "the database before the update" > "$STATE_DIR/5th-echelon.db"
systemctl start 5th-echelon; systemctl start 5th-echelon-standby
release_version=0.9.0 binary="" coord_binary="" auto_update="" work="$(mktemp -d)"
eval "$(fn install_updater)"
install_updater >/dev/null
update() { # update VERSION: asks for it, runs the updater, says what it recorded
  echo "$1" > "$STATE_DIR/update-request"
  : > /tmp/systemctl.log
  bash "$UPDATER" > /tmp/update.log 2>&1 || true
  echo "  $1: $(head -c 1024 "$UPDATE_DIR/update-status.json" 2>/dev/null)"
}
status_is() { [[ "$(head -c 1024 "$UPDATE_DIR/update-status.json")" == *"\"state\":\"$1\""* ]]; }
release_is() { [ "$(cat "$PROGRAM_DIR/release")" = "$1" ]; }

update 0.9.1
check "a signed release installs and comes back healthy" status_is "done"
check "  it records the release, and the one before as the oldest allowed" bash -c "[ \"\$(cat $PROGRAM_DIR/release)\" = 0.9.1 ] && [ \"\$(cat $ETC_DIR/min-release)\" = 0.9.0 ]"
check "  the server runs it" bash -c "curl -fs http://127.0.0.1/api/info | grep -q '\"version\":\"0.9.1\"'"
check "  a standby's coordinator is updated, not started" bash -c "grep -q 0.9.1 $PROGRAM_DIR/coordinator && ! systemctl is-active --quiet 5th-echelon-coordinator"
check "  the standby was stopped for it, and started again" bash -c "grep -q 'stop 5th-echelon-standby' /tmp/systemctl.log && systemctl is-active --quiet 5th-echelon-standby"
check "  the databases were copied first" test -f "$UPDATE_DIR/databases$STATE_DIR/5th-echelon.db"

update 0.9.2
check "a release that doesn't come back healthy is rolled back" status_is rolled-back
check "  the release before runs again" bash -c "[ \"\$(cat $PROGRAM_DIR/release)\" = 0.9.1 ] && curl -fs http://127.0.0.1/api/info | grep -q '\"version\":\"0.9.1\"'"
check "  with the database as it was before" bash -c "grep -q 'before the update' $STATE_DIR/5th-echelon.db"

update 0.9.2
check "the same failed release isn't tried again within the hour" bash -c "! grep -q 'stop 5th-echelon\$' /tmp/systemctl.log && [ \"\$(cat $PROGRAM_DIR/release)\" = 0.9.1 ]"

update 0.9.3
check "a release signed by another key is refused" bash -c "grep -q \"isn't signed by the release key\" $UPDATE_DIR/update-status.json && [ \"\$(cat $PROGRAM_DIR/release)\" = 0.9.1 ]"

update 0.8.0
check "a release older than the oldest allowed is refused" bash -c "grep -q \"won't install a release older than 0.9.0\" $UPDATE_DIR/update-status.json"

update 0.9.0
check "back to the release before, within the rollback window" bash -c "grep -q '\"state\":\"done\"' $UPDATE_DIR/update-status.json && [ \"\$(cat $PROGRAM_DIR/release)\" = 0.9.0 ]"
if [ "$rc" -ne 0 ]; then echo "--- the updater's last run"; cat /tmp/update.log; fi
systemctl stop 5th-echelon; systemctl stop 5th-echelon-standby

# --- backup.sh ----------------------------------------------------------------

echo "--- backup.sh"
mkdir -p /tmp/s3/live /tmp/s3/archive
rclone --config /dev/null serve s3 /tmp/s3 --auth-key testkey,testsecret123 --addr 127.0.0.1:9000 >/tmp/s3.log 2>&1 &
sleep 3
cat > "$BACKUP_ENV" <<ENV
BACKUP_R2_ENDPOINT=http://127.0.0.1:9000
BACKUP_R2_BUCKET=live
BACKUP_R2_ACCESS_KEY_ID=testkey
BACKUP_R2_SECRET_ACCESS_KEY=testsecret123
BACKUP_B2_ENDPOINT=http://127.0.0.1:9000
BACKUP_B2_BUCKET=archive
BACKUP_B2_ACCESS_KEY_ID=testkey
BACKUP_B2_SECRET_ACCESS_KEY=testsecret123
ENV
chmod 600 "$BACKUP_ENV"
export RCLONE_CONFIG_T_TYPE=s3 RCLONE_CONFIG_T_PROVIDER=Other RCLONE_CONFIG_T_ENDPOINT=http://127.0.0.1:9000 \
  RCLONE_CONFIG_T_ACCESS_KEY_ID=testkey RCLONE_CONFIG_T_SECRET_ACCESS_KEY=testsecret123
eval "$(fn backup_name)"
eval "$(fn write_backup_script)"
# Each machine's backup.sh, as the installer writes it: eu1 runs the coordinator, na1 is
# its standby.
machine() { # machine NAME COORD_ADDR
  COORD_DIR="/var/lib/$1-coord" COORD_SERVICE="$1-coord" BACKUP_SCRIPT="/tmp/$1-backup.sh" standby_me="$1" BACKUP_STATE="/tmp/$1-bstate"
  mkdir -p "$COORD_DIR" "$BACKUP_STATE"
  touch "/etc/systemd/system/$COORD_SERVICE.service"
  write_backup_script
  # The coordinator, and its live copy (PartOf it, as the installer's unit).
  cat > "/tmp/svc-$1-coord.cmd" <<CMD
fes-coordinator --listen $2:8700 --data $COORD_DIR &
for _ in \$(seq 50); do [ -s $COORD_DIR/coordinator.db ] && break; sleep 0.2; done
cat > /tmp/$1-ls.yml <<CFG
dbs:
  - path: $COORD_DIR/coordinator.db
    replica:
      type: s3
      endpoint: http://127.0.0.1:9000
      region: auto
      bucket: live
      path: live/$1/coordinator
      access-key-id: testkey
      secret-access-key: testsecret123
CFG
litestream replicate -config /tmp/$1-ls.yml &
wait
CMD
}
machine eu1 127.0.0.2
machine na1 127.0.0.3
# The game database's live copy, as the installer's unit runs it (this machine's: eu1).
python3 -c '
import sqlite3; c = sqlite3.connect("/var/lib/5th-echelon/game.db"); c.execute("PRAGMA journal_mode=WAL")
c.execute("CREATE TABLE users (name TEXT)"); c.execute("INSERT INTO users VALUES (\"Kiwi\")"); c.commit()'
rm -f "$STATE_DIR/5th-echelon.db"; mv /var/lib/5th-echelon/game.db "$STATE_DIR/5th-echelon.db"
touch "/etc/systemd/system/$SERVICE.service"
cat > /tmp/game-ls.yml <<CFG
dbs:
  - path: $STATE_DIR/5th-echelon.db
    replica:
      type: s3
      endpoint: http://127.0.0.1:9000
      region: auto
      bucket: live
      path: live/eu1/game
      access-key-id: testkey
      secret-access-key: testsecret123
CFG
echo "exec litestream replicate -config /tmp/game-ls.yml" > /tmp/svc-5th-echelon.cmd
systemctl start 5th-echelon
systemctl start eu1-coord
token="$(cat /var/lib/eu1-coord/join-token.txt)"
curl -fs -X POST http://127.0.0.2:8700/v1/join -H 'content-type: application/json' -d "{\"token\":\"$token\",\"server_id\":\"testserver1\"}" >/dev/null
mkdir -p /var/lib/eu1-coord/reports/1 && echo "a report" > /var/lib/eu1-coord/reports/1/report.txt
sleep 4
users() { python3 -c 'import sqlite3,sys; print(",".join(r[0] for r in sqlite3.connect(sys.argv[1]).execute("SELECT name FROM users")))' "$STATE_DIR/5th-echelon.db"; }
kiwi_back() { [ "$(users)" = Kiwi ]; }
missing_archive_refused() { ! /tmp/eu1-backup.sh restore game --archive 2001-01-01 >/dev/null 2>&1 && kiwi_back; }
check "the coordinator's files go to R2 where it runs" bash -c "/tmp/eu1-backup.sh files >/dev/null && rclone --config /dev/null lsf -R T:live/live/eu1/coordinator-files | grep -q join-token.txt"
check "  and nowhere on a standby (it isn't running there)" bash -c "/tmp/na1-backup.sh files && ! rclone --config /dev/null lsf T:live/live/na1 2>/dev/null | grep -q ."
# The coordinator's user owns its folder; the backup runs as root. Links there never take
# root's files (the backup keys) to R2, whenever they're made.
mkdir -p /var/lib/eu1-coord/reports/2 && ln -s /etc/5th-echelon /var/lib/eu1-coord/reports/2/keys && ln -s "$BACKUP_ENV" /var/lib/eu1-coord/reports/2/env
check "  links in the reports are copied as links, never what they point at" bash -c "/tmp/eu1-backup.sh files >/dev/null 2>&1 && rclone --config /dev/null lsf -R T:live/live/eu1/coordinator-files | grep -q report.txt && ! rclone --config /dev/null cat T:live/live/eu1/coordinator-files 2>/dev/null | grep -q BACKUP_R2"
rm -rf /var/lib/eu1-coord/reports/2 /var/lib/eu1-coord/reports/1
mv /var/lib/eu1-coord/reports /var/lib/eu1-coord/reports.real && ln -s /etc/5th-echelon /var/lib/eu1-coord/reports
check "  a reports folder that is a link isn't followed" bash -c "/tmp/eu1-backup.sh files 2>&1 | grep -q 'is a link; not backed up' && ! rclone --config /dev/null cat T:live/live/eu1/coordinator-files 2>/dev/null | grep -q BACKUP_R2"
rm /var/lib/eu1-coord/reports && mv /var/lib/eu1-coord/reports.real /var/lib/eu1-coord/reports
mkdir -p /var/lib/eu1-coord/reports/3 && echo "another report" > /var/lib/eu1-coord/reports/3/report.txt
check "  and once it's a folder again, the reports go up, the deleted one gone" bash -c "/tmp/eu1-backup.sh files >/dev/null && rclone --config /dev/null lsf -R T:live/live/eu1/coordinator-files | grep -q '^reports/3/report.txt' && ! rclone --config /dev/null lsf -R T:live/live/eu1/coordinator-files | grep -q '^reports/1/'"
check "the daily archive goes to B2" bash -c "/tmp/eu1-backup.sh archive >/dev/null && rclone --config /dev/null lsf -R T:archive | grep -q \"$(date -u +%F)-game.db.gz\""
before="$(date -u +%FT%TZ)"
sleep 2
python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute("DELETE FROM users"); c.commit()' "$STATE_DIR/5th-echelon.db"
sleep 3
/tmp/eu1-backup.sh restore game --time "$before" >/dev/null
check "a database restores as it was at a time" kiwi_back
check "  the one it replaced is kept beside it" bash -c "ls $STATE_DIR | grep -q before-restore"
check "a missing archive changes nothing" missing_archive_refused
python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute("DELETE FROM users"); c.commit()' "$STATE_DIR/5th-echelon.db"
/tmp/eu1-backup.sh restore game --archive "$(date -u +%F)" >/dev/null
check "a database restores from the archive" kiwi_back
systemctl stop 5th-echelon

# --- Failover -------------------------------------------------------------------

echo "--- failover"
# Cloudflare: the DNS API for one zone, and the proxy, which sends /probe/... to the
# coordinator the play.test record points at (521 when it doesn't answer).
cat > /tmp/cf.py <<'PY'
import json, http.server, urllib.request, urllib.parse
STATE = "/tmp/records.json"
ORIGIN = {"192.0.2.1": "127.0.0.2", "192.0.2.2": "127.0.0.3"}
def load(): return json.load(open(STATE))
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, body):
        b = json.dumps(body).encode()
        self.send_response(code); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        if u.path.startswith("/probe/"):
            rec = [r for r in load() if r["name"] == "play.test"][0]
            try:
                with urllib.request.urlopen(f"http://{ORIGIN[rec['content']]}:8700{u.path[len('/probe'):]}", timeout=3) as r:
                    return self.send(r.status, json.loads(r.read()))
            except Exception:
                return self.send(521, {"error": "Web server is down"})
        if self.headers.get("authorization") != "Bearer tok":
            return self.send(403, {"success": False, "errors": [{"code": 9109, "message": "Invalid access token"}]})
        name = urllib.parse.parse_qs(u.query).get("name", [""])[0]
        self.send(200, {"success": True, "result": [r for r in load() if r["name"] == name]})
    def do_PATCH(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        recs = load()
        for r in recs:
            if r["id"] == self.path.rsplit("/", 1)[1]: r["content"] = body["content"]
        json.dump(recs, open(STATE, "w"))
        self.send(200, {"success": True, "result": {}})
http.server.ThreadingHTTPServer(("127.0.0.1", 8443), H).serve_forever()
PY
echo '[{"id":"r1","name":"play.test","type":"A","content":"192.0.2.1","proxied":true},{"id":"r2","name":"metrics.test","type":"A","content":"192.0.2.1","proxied":true}]' > /tmp/records.json
python3 /tmp/cf.py >/tmp/cf.log 2>&1 &
printf 'CLOUDFLARE_API_TOKEN=tok\nCLOUDFLARE_ZONE_ID=z\n' > "$STANDBY_ENV"; chmod 600 "$STANDBY_ENV"
for m in eu1:192.0.2.1:127.0.0.2 na1:192.0.2.2:127.0.0.3; do
  IFS=: read -r name ip local <<< "$m"
  cat > "/tmp/$name.conf" <<CONF
record play.test
record metrics.test
me $name
address $ip
server eu1 192.0.2.1
server na1 192.0.2.2
secrets $STANDBY_ENV
state /tmp/$name-state
service $name-coord
local http://$local:8700
database /var/lib/$name-coord/coordinator.db
backup /tmp/$name-backup.sh
api http://127.0.0.1:8443/client/v4
probe http://127.0.0.1:8443/probe/v1/info
wait 10
CONF
  echo "exec fes-coordinator standby --config /tmp/$name.conf" > "/tmp/svc-$name-standby.cmd"
done
records() { python3 -c 'import json; print(" ".join(r["content"] for r in json.load(open("/tmp/records.json"))))'; }
servers() { curl -fs "http://$1:8700/v1/info" | python3 -c 'import json,sys; print(json.load(sys.stdin)["servers"])'; }
wait_for() { for _ in $(seq "$1"); do if eval "$2"; then return 0; fi; sleep 1; done; return 1; }
took_eu1s_data() { [ "$(servers 127.0.0.3)" = 1 ] && [ "$(cat /var/lib/na1-coord/join-token.txt)" = "$token" ] && [ -f /var/lib/na1-coord/reports/1/report.txt ]; }
moved_back() { [ "$(records)" = "192.0.2.1 192.0.2.1" ] && [ "$(servers 127.0.0.2)" = 2 ]; }
systemctl start eu1-standby; systemctl start na1-standby
sleep 5
check "the standby keeps the coordinator where its record points" bash -c "systemctl is-active --quiet eu1-coord && ! systemctl is-active --quiet na1-coord"
# eu1 goes down, machine and all.
systemctl stop eu1-standby; systemctl stop eu1-coord
check "a standby takes over a coordinator that stays down, and moves both records" wait_for 120 '[ "$(records)" = "192.0.2.2 192.0.2.2" ]'
check "  with its data, join token and reports, from the live backup" took_eu1s_data
check "  answering through the proxy" bash -c "curl -fs http://127.0.0.1:8443/probe/v1/info | grep -q coordinator"
# eu1 comes back, its coordinator started as before failover.
systemctl start eu1-coord; systemctl start eu1-standby
check "the one replaced stands down when it's back" wait_for 40 '! systemctl is-active --quiet eu1-coord'
curl -fs -X POST http://127.0.0.3:8700/v1/join -H 'content-type: application/json' -d "{\"token\":\"$token\",\"server_id\":\"testserver2\"}" >/dev/null
/tmp/na1-backup.sh files >/dev/null
sleep 3
systemctl stop eu1-standby
fes-coordinator standby --config /tmp/eu1.conf --take-over >/tmp/take-over.log 2>&1 || true
systemctl start eu1-standby
check "--take-over moves it back, with what changed meanwhile" moved_back
check "  and the other stands down" wait_for 40 '! systemctl is-active --quiet na1-coord'
if [ "$rc" -ne 0 ]; then
  for m in na1 eu1; do echo "--- $m standby"; tail -20 "/tmp/svc-$m-standby.log" 2>/dev/null; done
  echo "--- take-over"; cat /tmp/take-over.log
fi
# The scripts the installer writes, checked like the rest (when ShellCheck is here).
if command -v shellcheck >/dev/null; then
  check "the generated updater and backup.sh pass ShellCheck" shellcheck -S warning "$UPDATER" /tmp/eu1-backup.sh
fi
exit "$rc"
