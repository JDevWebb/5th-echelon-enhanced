#!/usr/bin/env bash
# How well the relays cover where players are, read-only (0.5.6's RL): from launchers' pings to
# every relay (the coordinator's `player_pings` rows with a batch: one launcher start each).
#
# - Per country: starts, the most picked relay and the median of the best ping, the median of
#   the second best, and the share of starts whose best relay is 80 ms or more away.
# - Per relay: the starts whose nearest it was, their median ping to it, and their median to
#   the next best: what those players would have without it (RN's "worth" of a relay).
#
#   scripts/relay-coverage.sh <ssh host> [days, default 14] [database, default /var/lib/5th-echelon-coordinator/coordinator.db]
#   scripts/relay-coverage.sh - [days] <database>   (a local copy)
#
# Run it before retiring a relay and two weeks after. Opens the database read-only over SSH
# (sqlite3 on the server); changes nothing. Places are a city and country only (no addresses
# are kept).
set -euo pipefail
host=${1:?ssh host of the coordinator}
days=${2:-14}
db=${3:-/var/lib/5th-echelon-coordinator/coordinator.db}
case $days in *[!0-9]* | '') echo "days must be a number" >&2; exit 2 ;; esac

# Each start's relays ranked by ping (1 = nearest), with the relay's name from its listing.
sql=$(cat <<SQL
.headers on
.mode column
.print By country
with p as (
  select p.batch, p.country, p.server_id, p.ms,
    coalesce(json_extract(s.listing,'\$.name'), p.server_id) relay,
    row_number() over (partition by p.batch order by p.ms) rank
  from player_pings p join servers s on s.id = p.server_id
  where p.batch <> '' and p.at >= strftime('%s','now','-$days days')
), starts as (
  select b.batch, b.country, b.relay best, b.ms best_ms, n.relay next, n.ms next_ms
  from p b left join p n on n.batch = b.batch and n.rank = 2
  where b.rank = 1
), ranked as (
  select *, row_number() over (partition by country order by best_ms) bi,
    row_number() over (partition by country order by coalesce(next_ms, 99999)) ni,
    count(*) over (partition by country) n
  from starts
)
select country, n starts,
  (select best from starts s where s.country = r.country group by best order by count(*) desc limit 1) most_picked,
  max(case when bi = (n + 1) / 2 then best_ms end) best_median_ms,
  max(case when ni = (n + 1) / 2 then next_ms end) next_median_ms,
  printf('%d%%', round(100.0 * sum(best_ms >= 80) / n)) best_80ms_plus
from ranked r group by country order by n desc;

.print
.print By relay

with p as (
  select p.batch, p.server_id, p.ms,
    coalesce(json_extract(s.listing,'\$.name'), p.server_id) relay,
    row_number() over (partition by p.batch order by p.ms) rank
  from player_pings p join servers s on s.id = p.server_id
  where p.batch <> '' and p.at >= strftime('%s','now','-$days days')
), starts as (
  select b.relay, b.ms best_ms, n.relay next, n.ms next_ms
  from p b left join p n on n.batch = b.batch and n.rank = 2
  where b.rank = 1
), ranked as (
  select *, row_number() over (partition by relay order by best_ms) bi,
    row_number() over (partition by relay order by coalesce(next_ms, 99999)) ni,
    count(*) over (partition by relay) n
  from starts
)
select relay, n nearest_for,
  max(case when bi = (n + 1) / 2 then best_ms end) median_ms,
  (select next from starts s where s.relay = r.relay and next is not null group by next order by count(*) desc limit 1) else_mostly,
  max(case when ni = (n + 1) / 2 then next_ms end) else_median_ms
from ranked r group by relay order by n desc;
SQL
)
echo "Launcher starts' pings to the relays, last $days days ($host)"
echo
if [ "$host" = - ]; then
  sqlite3 -readonly "file:$db?mode=ro" <<<"$sql"
else
  # shellcheck disable=SC2029 # the query is built here on purpose
  ssh -o BatchMode=yes "$host" "sudo sqlite3 -readonly 'file:$db?mode=ro'" <<<"$sql"
fi
