#!/usr/bin/env bash
# How joins into match rooms went on a server, read-only (0.5.5's JC5): joins that got in and
# connection failures (Storm's CONNECTION_FAILED 0xb08a1a05, and the add-on's "never got in"), by
# whether the joiner and the host were relayed (each one's last `nat` event before the join), the
# direct hosts most joins failed into, the moves to the relay (J2, JC1, JC4) and the peers games
# couldn't reach (JC4), and players' address changes during a sign-in (IP) with the joins and
# drops around them. Compare a week before and after a release.
#
#   scripts/join-stats.sh <ssh host> [days, default 7] [database, default /var/lib/5th-echelon/5th-echelon.db]
#
# Opens the database read-only over SSH (sqlite3 on the server); changes nothing.
set -euo pipefail
host=${1:?ssh host of the network matchmaking server}
days=${2:-7}
db=${3:-/var/lib/5th-echelon/5th-echelon.db}
case $days in *[!0-9]* | '') echo "days must be a number" >&2; exit 2 ;; esac

sql=$(cat <<SQL
.headers on
.mode column
with ev as (
  select e.at, e.name, e.kind, json_extract(e.detail,'\$.host_name') host,
    case when e.kind='join' then 'ok' else json_extract(e.detail,'\$.code') end res
  from session_events e
  where e.at >= strftime('%s','now','-$days days') and json_extract(e.detail,'\$.room_kind')='match'
    and ((e.kind='join' and coalesce(json_extract(e.detail,'\$.via'),'')<>'migration')
      or (e.kind='join_failed' and json_extract(e.detail,'\$.code') in ('0xb08a1a05','0x00000000')
          and coalesce(json_extract(e.detail,'\$.own'),0)=0))
), r as (
  select ev.*,
    (select json_extract(n.detail,'\$.relayed')||'@'||coalesce(json_extract(n.detail,'\$.relay'),'-') from session_events n
       where n.name=ev.name and n.kind='nat' and n.at<=ev.at order by n.at desc limit 1) j,
    (select json_extract(n.detail,'\$.relayed')||'@'||coalesce(json_extract(n.detail,'\$.relay'),'-') from session_events n
       where n.name=ev.host and n.kind='nat' and n.at<=ev.at order by n.at desc limit 1) h
  from ev
)
select case when j is null or h is null then 'unknown'
  when j like '0@%' and h like '0@%' then 'direct -> direct'
  when j like '1@%' and h like '1@%' and substr(j,3)=substr(h,3) then 'relayed -> relayed, same relay'
  when j like '1@%' and h like '1@%' then 'relayed -> relayed, other relay'
  when j like '1@%' then 'relayed -> direct'
  else 'direct -> relayed' end as "joiner -> host",
  sum(res='ok') as "in", sum(res<>'ok') as failed, round(100.0*sum(res<>'ok')/count(*),1) as "failed %"
from r group by 1 order by 3 desc;
.print
.print Direct hosts most joins failed into:
with ev as (
  select e.at, e.name, json_extract(e.detail,'\$.host_name') host, case when e.kind='join' then 1 else 0 end ok
  from session_events e
  where e.at >= strftime('%s','now','-$days days') and json_extract(e.detail,'\$.room_kind')='match'
    and ((e.kind='join' and coalesce(json_extract(e.detail,'\$.via'),'')<>'migration')
      or (e.kind='join_failed' and json_extract(e.detail,'\$.code') in ('0xb08a1a05','0x00000000')
          and coalesce(json_extract(e.detail,'\$.own'),0)=0))
)
select host, sum(ok) as "in", sum(1-ok) as failed, count(distinct case when ok=0 then name end) as "players failing"
from ev where (select json_extract(n.detail,'\$.relayed') from session_events n where n.name=ev.host and n.kind='nat' and n.at<=ev.at order by n.at desc limit 1)=0
group by host having failed > 0 order by failed desc limit 10;
.print
.print Why players were relayed or direct (their nat events):
select coalesce(json_extract(detail,'\$.why'),'(not said)') as why, count(*) as registrations, count(distinct name) as players
from session_events where kind='nat' and at >= strftime('%s','now','-$days days') group by 1 order by 2 desc;
.print
.print "Peers games couldn't reach (JC4):"
select case when json_extract(detail,'\$.found') then 'found' else 'not found' end as peer,
  coalesce(json_extract(detail,'\$.together'),0) as together, count(*) as reports, count(distinct name) as reporters,
  count(distinct json_extract(detail,'\$.peer')) as peers
from session_events where kind='unreachable' and at >= strftime('%s','now','-$days days') group by 1, 2;
.print
.print Address changes during a sign-in (0.5.6's IP), by network and outcome:
select json_extract(detail,'\$.net') as net, json_extract(detail,'\$.outcome') as outcome, sum(count) as changes, count(distinct user_id) as players
from session_events where kind='address_change' and at >= strftime('%s','now','-$days days') group by 1, 2 order by 3 desc;
.print
.print Failed joins, relay drops and removals within a minute of an address change:
select e.kind || coalesce(' ' || json_extract(e.detail,'\$.how'),'') as what, count(*) as events, count(distinct e.user_id) as players
from session_events e
where e.at >= strftime('%s','now','-$days days')
  and (e.kind in ('join_failed','relay_drop') or (e.kind='leave' and json_extract(e.detail,'\$.how') in ('removed','timed_out')))
  and exists (select 1 from session_events a where a.kind='address_change' and a.user_id=e.user_id and e.at between a.at-60 and a.last_at+60)
group by 1 order by 2 desc;
SQL
)
echo "# $host, the last $days days"
ssh -o BatchMode=yes "$host" "sudo sqlite3 -readonly 'file:$db?mode=ro'" <<<"$sql"
