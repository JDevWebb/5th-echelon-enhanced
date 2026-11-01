//! The host-migration cases beyond `host-migration*` and `party-*` in `main.rs`: one scenario
//! per case of the case list in `docs/architecture/host-and-party-migration.md` that the server
//! takes part in.

use super::*;

/// A public match lobby: matchmaking finds it.
const PUBLIC: &str = "113 => 0;103 => 0;102 => 3;3 => 4;4 => 0";
/// What a matchmaking search for it asks.
const PUBLIC_QUERY: &str = "113 => 0;102 => 3";

fn sorted(mut pids: Vec<u32>) -> Vec<u32> {
    pids.sort_unstable();
    pids
}

/// Case 9, and 13: a host that goes quiet (its game killed, its connection not yet expired)
/// while a member's split waits for it. After 20 s of silence (two missed pings) the takeover
/// goes through: the others move into the member's room and are told it moved (7001); and a
/// lobby of a host that quiet isn't offered by matchmaking until it speaks again. A host that
/// keeps talking is never taken over, however long the member's split waits.
pub async fn host_migration_silent(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut heir = ctx.player("Guest").await?;
    let mut other = ctx.player("Guest").await?;
    let mut busy = ctx.player("Host").await?;
    let mut leaver = ctx.player("Guest").await?;
    let mut stays = ctx.player("Guest").await?;
    let mut seeker = ctx.player("Seeker").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    busy.register_urls(&["prudp:/address=127.0.0.1;port=3075;sid=15;type=3"]).await?;

    let game = host.create_session(PUBLIC).await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    heir.join_session(game).await?;
    other.join_session(game).await?;
    let lobby = host.create_session(PUBLIC).await?;
    host.add_participants(lobby, &[], &[host.pid]).await?;
    let kept = busy.create_session(PUBLIC).await?;
    busy.add_participants(kept, &[], &[busy.pid]).await?;
    leaver.join_session(kept).await?;
    stays.join_session(kept).await?;
    let offered = seeker.search_sessions(PUBLIC_QUERY).await?;
    ensure!(offered.iter().any(|(s, _)| *s == lobby), "the quiet host's lobby isn't offered to begin with: {offered:?}");

    // Both hosts still signed in: each member's split waits for its host.
    let copy = heir.split_session(game).await?;
    heir.add_participants(copy, &[], &[heir.pid]).await?;
    let own = leaver.split_session(kept).await?;
    leaver.add_participants(own, &[], &[leaver.pid]).await?;
    // The host's last word.
    host.update_session(lobby, PUBLIC).await?;
    let quiet_since = tokio::time::Instant::now();

    let deadline = quiet_since + Duration::from_secs(35);
    let (mut moved_at, mut hidden_at) = (None, None);
    let mut seen = (None, None);
    while moved_at.is_none() || hidden_at.is_none() {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "after 35 s of the host's silence: moved {moved_at:?}, lobby hidden {hidden_at:?}; the heir's room {:?}, the match {:?}",
            seen.0,
            seen.1
        );
        // The busy host keeps talking, as a game's pings do.
        busy.update_session(kept, PUBLIC).await?;
        let found = seeker.search_with_participants(&[other.pid, heir.pid]).await?;
        seen = (room_in(&found, copy), room_in(&found, game));
        if moved_at.is_none() && seen.0 == Some((heir.pid, sorted(vec![heir.pid, other.pid]))) {
            moved_at = Some(quiet_since.elapsed());
        }
        if hidden_at.is_none() && !seeker.search_sessions(PUBLIC_QUERY).await?.iter().any(|(s, _)| *s == lobby) {
            hidden_at = Some(quiet_since.elapsed());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let (moved_at, hidden_at) = (moved_at.unwrap_or_default(), hidden_at.unwrap_or_default());
    ensure!(moved_at >= Duration::from_secs(18), "the takeover went through after only {moved_at:?} of silence");
    ensure!(hidden_at >= Duration::from_secs(18), "the quiet host's lobby was hidden after only {hidden_at:?}");
    let mut told = false;
    while let Some(n) = other.wait_notification(Duration::from_secs(2)).await? {
        told |= n.ui_type == 7001 && n.ui_param_1 == copy && n.ui_param_2 == game && n.pid_source == heir.pid;
    }
    ensure!(told, "the other guest wasn't told its match moved to the heir's");
    // The busy host's match kept its guest.
    let found = seeker.search_with_participants(&[stays.pid]).await?;
    ensure!(
        room_in(&found, kept) == Some((busy.pid, sorted(vec![busy.pid, stays.pid]))),
        "a host that kept talking lost its guest to a member's split: {:?} {:?}",
        room_in(&found, kept),
        room_in(&found, own)
    );
    // The quiet host speaks again: its lobby is offered again.
    host.update_session(lobby, PUBLIC).await?;
    ensure!(
        seeker.search_sessions(PUBLIC_QUERY).await?.iter().any(|(s, _)| *s == lobby),
        "the host's lobby isn't offered again once it speaks"
    );
    for bot in [host, heir, other, busy, leaver, stays, seeker] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Case 10: the new host goes too. The heir takes the party over, then leaves it, and another
/// member takes over the heir's room; the last one carries on with the rest. A member whose game
/// splits a room of the chain late (the first one, or the heir's) is answered with the room it's
/// in now, not a new one.
pub async fn host_migration_cascade(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut heir = ctx.player("Heir").await?;
    let mut next = ctx.player("Heir").await?;
    let mut last = ctx.player("Member").await?;
    let mut friend = ctx.player("Friend").await?;
    let party = host.create_session(LOBBY).await?;
    host.add_participants(party, &[host.pid], &[]).await?;
    for p in [&mut heir, &mut next, &mut last] {
        p.add_participants(party, &[p.pid], &[]).await?;
        p.join_session(party).await?;
    }
    host.abandon_session(party).await?;
    let first = heir.split_session(party).await?;
    heir.add_participants(first, &[heir.pid], &[]).await?;
    let found = friend.search_with_participants(&[last.pid]).await?;
    ensure!(
        room_in(&found, first) == Some((heir.pid, sorted(vec![heir.pid, next.pid, last.pid]))),
        "the first takeover didn't take everyone: {:?}",
        room_in(&found, first)
    );
    // The heir's game goes too.
    heir.abandon_session(first).await?;
    let second = next.split_session(first).await?;
    ensure!(second != first && second != party, "the second takeover made no new room");
    next.add_participants(second, &[next.pid], &[]).await?;
    let found = friend.search_with_participants(&[last.pid]).await?;
    ensure!(
        room_in(&found, second) == Some((next.pid, sorted(vec![next.pid, last.pid]))),
        "the second takeover didn't take the rest: {:?}",
        room_in(&found, second)
    );
    // Late splits of either room of the chain: the room the member is in.
    for (old, what) in [(party, "the first room"), (first, "the heir's room")] {
        let answer = last.split_session(old).await?;
        ensure!(answer == second, "a late split of {what} was answered with {answer}, not {second}");
    }
    let found = friend.search_with_participants(&[last.pid, heir.pid, host.pid]).await?;
    ensure!(room_in(&found, second).is_some_and(|(_, m)| m.contains(&last.pid)), "the member lost its room");
    for bot in [host, heir, next, last, friend] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Case 11: an invitation pending when the room's host goes follows the room. The game's own
/// invitation (SendInvitation) names the heir's room afterwards, from the heir; so does one sent
/// through the friend list (the overlay's or the launcher's invite), which the invited player's
/// game takes up then.
pub async fn host_migration_invite(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut heir = ctx.player("Heir").await?;
    let mut invited = ctx.player("Invited").await?;
    let befriended = ctx.player("Friend").await?;
    befriend(&host, &befriended).await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let game = host.create_session(PUBLIC).await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    host.set_session(game, false).await?;
    heir.join_session(game).await?;
    host.send_invitation(game, &[invited.pid], "come along").await?;
    host.invite(&befriended.name).await?;
    let got = invited.invitations_received().await?;
    ensure!(
        got.iter().any(|i| i.session_key.session_id == game && i.sender_pid == host.pid),
        "the game's invitation isn't there to begin with: {got:?}"
    );

    host.abandon_session(game).await?;
    let copy = heir.split_session(game).await?;
    heir.add_participants(copy, &[], &[heir.pid]).await?;
    let got = invited.invitations_received().await?;
    ensure!(
        got.iter().any(|i| i.session_key.session_id == copy && i.sender_pid == heir.pid),
        "the game's invitation doesn't name the heir's room, from the heir: {got:?} (room {copy})"
    );
    ensure!(!got.iter().any(|i| i.session_key.session_id == game), "the invitation still names the room the host left");
    let from = befriended.poll_invite(Duration::from_secs(3)).await?;
    ensure!(from.as_deref() == Some(heir.name.as_str()), "the friend list's invitation isn't the heir's now: {from:?}");
    for bot in [host, heir, invited, befriended] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Case 14: the host's game drops and signs in again from the same address while a member's
/// split waits for it. The rooms its old connection made have nobody behind them, though the
/// player talks on the new one: the takeover goes through once the old connection has said
/// nothing for 20 s, so the others follow the member who took over, long before that connection
/// expires (a minute) and its rooms end.
pub async fn host_migration_host_returns(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut heir = ctx.player("Heir").await?;
    let mut member = ctx.player("Member").await?;
    let mut friend = ctx.player("Friend").await?;
    host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
    let party = host.create_session(LOBBY).await?;
    host.add_participants(party, &[host.pid], &[]).await?;
    for p in [&mut heir, &mut member] {
        p.add_participants(party, &[p.pid], &[]).await?;
        p.join_session(party).await?;
    }
    let copy = heir.split_session(party).await?;
    heir.add_participants(copy, &[heir.pid], &[]).await?;
    let mut host = host.reconnect(ctx.server, PASSWORD, Duration::from_secs(1), false).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        // The returned game keeps talking, as its pings do.
        host.register_urls(&["prudp:/address=127.0.0.1;port=3074;sid=15;type=3"]).await?;
        let found = friend.search_with_participants(&[member.pid]).await?;
        if room_in(&found, copy) == Some((heir.pid, sorted(vec![heir.pid, member.pid]))) {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "after the host's game came back, the member didn't follow the heir: {:?} {:?}",
            room_in(&found, copy),
            room_in(&found, party)
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    for bot in [host, heir, member, friend] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Case 2: a public match whose players have parties of their own. The match moves to the heir;
/// every party stays as it was (the heir's own included), and nobody from a party who wasn't in
/// the match is taken into it.
pub async fn host_migration_parties(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut heir = ctx.player("Heir").await?;
    let mut heirs_mate = ctx.player("Mate").await?;
    let mut guest = ctx.player("Guest").await?;
    let mut guests_mate = ctx.player("Mate").await?;
    let mut friend = ctx.player("Friend").await?;
    let mut parties = vec![];
    for (leader, mate) in [(&mut heir, &mut heirs_mate), (&mut guest, &mut guests_mate)] {
        let party = leader.create_session(LOBBY).await?;
        leader.add_participants(party, &[leader.pid], &[]).await?;
        mate.add_participants(party, &[mate.pid], &[]).await?;
        mate.join_session(party).await?;
        parties.push((party, leader.pid, mate.pid));
    }
    let game = host.create_session(PUBLIC).await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    heir.join_session(game).await?;
    guest.join_session(game).await?;
    host.abandon_session(game).await?;
    let copy = heir.split_session(game).await?;
    heir.add_participants(copy, &[], &[heir.pid]).await?;
    let found = friend.search_with_participants(&[heir.pid, guest.pid, heirs_mate.pid, guests_mate.pid]).await?;
    ensure!(
        room_in(&found, copy) == Some((heir.pid, sorted(vec![heir.pid, guest.pid]))),
        "the match didn't move to the heir with the guest, and only them: {:?}",
        room_in(&found, copy)
    );
    for (party, leader, mate) in parties {
        ensure!(
            room_in(&found, party) == Some((leader, sorted(vec![leader, mate]))),
            "a party changed with the match's migration: {:?}",
            room_in(&found, party)
        );
    }
    for bot in [host, heir, heirs_mate, guest, guests_mate, friend] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Case 3: a private match isn't migrated. Its host ends it (DeleteSession); a guest's game that
/// splits it afterwards gets a room of its own, the other guest stays out of it, and nobody is
/// told the match moved.
pub async fn private_match_ends(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut first = ctx.player("Guest").await?;
    let mut second = ctx.player("Guest").await?;
    let mut friend = ctx.player("Friend").await?;
    for (bot, port) in [(&mut host, 3074), (&mut first, 3075), (&mut second, 3076)] {
        bot.register_urls(&[&format!("prudp:/address=127.0.0.1;port={port};sid=15;type=3")]).await?;
    }
    let game = host.create_session(PRIVATE_MATCH).await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    host.set_session(game, true).await?;
    for g in [&first, &second] {
        befriend(&host, g).await?;
    }
    host.add_participants(game, &[], &[first.pid, second.pid]).await?;
    for g in [&mut first, &mut second] {
        while g.wait_notification(Duration::from_millis(300)).await?.is_some() {}
    }
    host.delete_session(game).await?;
    // Refused, or a room of its own: either way the other guest stays out of it.
    if let Ok(own) = first.split_session(game).await {
        let found = friend.search_with_participants(&[second.pid, first.pid]).await?;
        ensure!(
            room_in(&found, own).is_none_or(|(_, members)| !members.contains(&second.pid)),
            "a guest's split of an ended private match took the other guest along: {:?}",
            room_in(&found, own)
        );
    }
    while let Some(n) = second.wait_notification(Duration::from_millis(500)).await? {
        ensure!(n.ui_type != 7001, "the other guest was told the ended private match moved: {n:?}");
    }
    for bot in [host, first, second, friend] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// Cases 7, 8 and 17: after a private match the party took its leader into, a member who stays
/// in the match (the leader left alone, "leave the party") isn't called back into the leader's
/// new party; one who left the match and is alone in the party's old room is.
pub async fn party_return_stays(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let mut leader = ctx.player("Leader").await?;
    let mut stays = ctx.player("Member").await?;
    let mut back = ctx.player("Member").await?;
    for (bot, port) in [(&mut host, 3074), (&mut leader, 3075), (&mut stays, 3076), (&mut back, 3077)] {
        bot.register_urls(&[&format!("prudp:/address=127.0.0.1;port={port};sid=15;type=3")]).await?;
    }
    befriend(&host, &stays).await?;
    befriend(&host, &back).await?;
    let lobby = host.create_session(LOBBY).await?;
    host.add_participants(lobby, &[host.pid], &[]).await?;
    host.set_session(lobby, true).await?;
    let game = host.create_session(PRIVATE_MATCH).await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    let party = leader.create_session(LOBBY).await?;
    leader.add_participants(party, &[leader.pid], &[]).await?;
    for p in [&mut stays, &mut back] {
        p.add_participants(party, &[p.pid], &[]).await?;
        p.join_session(party).await?;
    }
    host.invite(&leader.name).await?;
    ensure!(leader.poll_invite(Duration::from_secs(3)).await?.is_some(), "the invitation isn't delivered");
    ensure!(
        has_session(&leader.search_with_participants(&[host.pid]).await?, game),
        "the leader isn't answered with the match"
    );
    leader.add_participants(game, &[], &[leader.pid]).await?;
    host.add_participants(game, &[], &[stays.pid, back.pid])
        .await
        .map_err(|e| eyre!("the host couldn't take the party in: {e}"))?;

    // The leader leaves the match and its party; one member leaves the match too and splits
    // the party's old room alone; the other plays on with the host.
    leader.leave_session(game).await?;
    leader.abandon_session(party).await?;
    leader.leave_session(party).await?;
    back.leave_session(game).await?;
    let alone = back.split_session(party).await?;
    back.add_participants(alone, &[back.pid], &[]).await?;
    let again = leader.create_session(LOBBY).await?;
    leader.add_participants(again, &[leader.pid], &[]).await?;
    let got = back.poll_invite_event(Duration::from_secs(5)).await?;
    ensure!(
        got.as_ref().is_some_and(|(from, requested)| *from == leader.name && *requested),
        "the member who left the match isn't asked back: {got:?}"
    );
    ensure!(
        stays.poll_invite_event(Duration::from_secs(2)).await?.is_none(),
        "the member still playing in the host's match was called out of it"
    );
    for bot in [host, leader, stays, back] {
        bot.disconnect().await?;
    }
    Ok(())
}

/// The friend list says which friends are in a private match, not just in an invite-only room
/// (parties are announced invite-only too): what the leader's game reads to keep its party when
/// it accepts an invitation (`party_follow`).
pub async fn friend_list_private_match(ctx: &mut Ctx) -> Result<()> {
    let mut host = ctx.player("Host").await?;
    let viewer = ctx.player("Viewer").await?;
    befriend(&host, &viewer).await?;
    let entry = |list: Vec<(String, u32, bool, bool)>, name: &str| list.into_iter().find(|(n, ..)| n == name).map(|(_, s, i, p)| (s, i, p));
    let party = host.create_session(LOBBY).await?;
    host.add_participants(party, &[host.pid], &[]).await?;
    host.set_session(party, true).await?;
    ensure!(
        entry(viewer.friend_sessions().await?, &host.name) == Some((party, true, false)),
        "an invite-only party is shown as a private match, or not at all"
    );
    let game = host.create_session(PRIVATE_MATCH).await?;
    host.add_participants(game, &[], &[host.pid]).await?;
    host.set_session(game, true).await?;
    ensure!(
        entry(viewer.friend_sessions().await?, &host.name) == Some((game, true, true)),
        "a private match isn't shown as one"
    );
    let public = host.create_session(PUBLIC).await?;
    host.add_participants(public, &[], &[host.pid]).await?;
    host.set_session(public, false).await?;
    ensure!(
        entry(viewer.friend_sessions().await?, &host.name) == Some((public, false, false)),
        "a public match is shown as a private one"
    );
    host.disconnect().await?;
    viewer.disconnect().await
}
