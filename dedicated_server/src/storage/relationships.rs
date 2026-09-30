//! Friends, friend requests and blocks (the `relationships` table), what to
//! tell players about them (`friend_events`), and the identity links and
//! outbox that carry them to other servers (see `federation.rs`).

use eyre::eyre;

use super::name_key;
use super::Result;
use super::Storage;

/// How one player stands to another, from the first one's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    None,
    Friend,
    /// I asked them; they haven't answered.
    RequestSent,
    /// They asked me.
    RequestReceived,
    /// I blocked them.
    Blocked,
    /// They blocked me. Never shown to me: I see [`Relation::None`], or my
    /// request as sent.
    BlockedBy,
}

/// Why a friend change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FriendError {
    #[error("no such player")]
    NotFound,
    #[error("that's you")]
    Yourself,
    #[error("you blocked them; unblock them first")]
    YouBlocked,
    #[error("too many friend requests waiting for an answer")]
    TooManyRequests,
    #[error("they didn't ask to be your friend")]
    NoRequest,
}

/// Friend requests one player can have waiting for an answer.
const MAX_OPEN_REQUESTS: i64 = 100;

/// A player, as friend lists and searches show them.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Person {
    pub id: u32,
    pub username: String,
    pub ubi_id: String,
    pub is_online: bool,
    pub global_id: Option<String>,
}

const PERSON: &str = "u.id, u.username, u.ubi_id, u.is_online, u.global_id";

/// What happened, for [`Storage::take_friend_event`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriendEventKind {
    /// They sent me a friend request.
    Request,
    /// They accepted mine.
    Accepted,
}

impl FriendEventKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Accepted => "accepted",
        }
    }
}

impl Storage {
    /// How `me` stands to `other`.
    pub async fn relation(&self, me: u32, other: u32) -> Result<Relation> {
        let rows: Vec<(u32, String)> = sqlx::query_as("SELECT user_id, kind FROM relationships WHERE (user_id = ? AND other_id = ?) OR (user_id = ? AND other_id = ?)")
            .bind(me)
            .bind(other)
            .bind(other)
            .bind(me)
            .fetch_all(&self.pool)
            .await?;
        let mine = rows.iter().find(|(u, _)| *u == me).map(|(_, k)| k.as_str());
        let theirs = rows.iter().find(|(u, _)| *u == other).map(|(_, k)| k.as_str());
        Ok(match (mine, theirs) {
            (Some("block"), _) => Relation::Blocked,
            (_, Some("block")) => Relation::BlockedBy,
            (Some("friend"), _) => Relation::Friend,
            (Some("request"), _) => Relation::RequestSent,
            (_, Some("request")) => Relation::RequestReceived,
            _ => Relation::None,
        })
    }

    /// Accounts of players: not the server's own ones, nor the sample
    /// accounts that can't sign in.
    fn players() -> &'static str {
        "u.ubi_id IS NOT NULL AND (u.password IS NOT NULL OR u.password_hash IS NOT NULL)"
    }

    /// `me`'s friends, by name.
    pub async fn friends_of(&self, me: u32) -> Result<Vec<Person>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {PERSON} FROM relationships r JOIN users u ON u.id = r.other_id
             WHERE r.user_id = ? AND r.kind = 'friend' AND {} ORDER BY u.name_key",
            Self::players()
        ))
        .bind(me)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Everyone but `me`, leaving out anyone blocked either way.
    pub async fn everyone_for(&self, me: u32) -> Result<Vec<Person>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {PERSON} FROM users u WHERE {} AND u.id != ?
             AND NOT EXISTS (SELECT 1 FROM relationships r WHERE r.kind = 'block'
                 AND ((r.user_id = ? AND r.other_id = u.id) OR (r.user_id = u.id AND r.other_id = ?)))
             ORDER BY u.name_key",
            Self::players()
        ))
        .bind(me)
        .bind(me)
        .bind(me)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Requests waiting for `me` to answer (`incoming`), or for others to
    /// answer `me`. A request from someone `me` blocked isn't shown.
    pub async fn friend_requests(&self, me: u32, incoming: bool) -> Result<Vec<Person>> {
        let sql = if incoming {
            format!(
                "SELECT {PERSON} FROM relationships r JOIN users u ON u.id = r.user_id
                 WHERE r.other_id = ? AND r.kind = 'request'
                 AND NOT EXISTS (SELECT 1 FROM relationships b WHERE b.user_id = r.other_id AND b.other_id = r.user_id AND b.kind = 'block')
                 ORDER BY r.created_at"
            )
        } else {
            format!("SELECT {PERSON} FROM relationships r JOIN users u ON u.id = r.other_id WHERE r.user_id = ? AND r.kind = 'request' ORDER BY r.created_at")
        };
        Ok(sqlx::query_as(&sql).bind(me).fetch_all(&self.pool).await?)
    }

    /// Who `me` has blocked.
    pub async fn blocked_by(&self, me: u32) -> Result<Vec<Person>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {PERSON} FROM relationships r JOIN users u ON u.id = r.other_id WHERE r.user_id = ? AND r.kind = 'block' ORDER BY u.name_key"
        ))
        .bind(me)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Players whose name contains `query` (any case), or, for an empty
    /// query, who's online; at most `limit`. Leaves out `me` and anyone
    /// blocked either way.
    pub async fn search_players(&self, me: u32, query: &str, limit: u32) -> Result<Vec<(Person, Relation)>> {
        let query = name_key(query);
        let people: Vec<Person> = if query.is_empty() {
            sqlx::query_as(&format!(
                "SELECT {PERSON} FROM users u WHERE {} AND u.id != ? AND u.is_online = 1 ORDER BY u.name_key LIMIT ?",
                Self::players()
            ))
            .bind(me)
            .bind(limit * 2)
            .fetch_all(&self.pool)
            .await?
        } else {
            // Names that start with the query first, then the rest.
            let pattern = format!("%{}%", query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            let prefix = format!("{}%", query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            sqlx::query_as(&format!(
                "SELECT {PERSON} FROM users u WHERE {} AND u.id != ? AND u.name_key LIKE ? ESCAPE '\\'
                 ORDER BY u.name_key NOT LIKE ? ESCAPE '\\', u.is_online DESC, u.name_key LIMIT ?",
                Self::players()
            ))
            .bind(me)
            .bind(pattern)
            .bind(prefix)
            .bind(limit * 2)
            .fetch_all(&self.pool)
            .await?
        };
        let mut found = Vec::new();
        for person in people {
            let relation = self.relation(me, person.id).await?;
            if !matches!(relation, Relation::Blocked | Relation::BlockedBy) {
                found.push((person, relation));
            }
            if found.len() >= limit as usize {
                break;
            }
        }
        Ok(found)
    }

    /// A player by name, whatever its case.
    pub async fn find_person_by_name(&self, username: &str) -> Result<Option<Person>> {
        Ok(sqlx::query_as(&format!("SELECT {PERSON} FROM users u WHERE u.name_key = ? AND {}", Self::players()))
            .bind(name_key(username))
            .fetch_optional(&self.pool)
            .await?)
    }

    /// A player by their account id (the game's "Ubisoft id").
    pub async fn find_person_by_ubi_id(&self, ubi_id: &str) -> Result<Option<Person>> {
        Ok(sqlx::query_as(&format!("SELECT {PERSON} FROM users u WHERE u.ubi_id = ?"))
            .bind(ubi_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn find_person(&self, id: u32) -> Result<Option<Person>> {
        Ok(sqlx::query_as(&format!("SELECT {PERSON} FROM users u WHERE u.id = ?")).bind(id).fetch_optional(&self.pool).await?)
    }

    /// `me` asks `other` to be friends. If `other` had asked `me`, they're
    /// friends now. A request to someone who blocked `me` looks sent but
    /// goes nowhere.
    pub async fn request_friend(&self, me: u32, other: u32) -> Result<std::result::Result<Relation, FriendError>> {
        if me == other {
            return Ok(Err(FriendError::Yourself));
        }
        match self.relation(me, other).await? {
            Relation::Friend => Ok(Ok(Relation::Friend)),
            Relation::RequestSent | Relation::BlockedBy => Ok(Ok(Relation::RequestSent)),
            Relation::Blocked => Ok(Err(FriendError::YouBlocked)),
            Relation::RequestReceived => {
                self.make_friends(me, other).await?;
                self.push_friend_event(other, me, FriendEventKind::Accepted).await?;
                Ok(Ok(Relation::Friend))
            }
            Relation::None => {
                let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM relationships WHERE user_id = ? AND kind = 'request'")
                    .bind(me)
                    .fetch_one(&self.pool)
                    .await?;
                if open >= MAX_OPEN_REQUESTS {
                    return Ok(Err(FriendError::TooManyRequests));
                }
                sqlx::query("INSERT INTO relationships (user_id, other_id, kind) VALUES (?, ?, 'request')")
                    .bind(me)
                    .bind(other)
                    .execute(&self.pool)
                    .await?;
                self.push_friend_event(other, me, FriendEventKind::Request).await?;
                Ok(Ok(Relation::RequestSent))
            }
        }
    }

    /// `me` accepts `other`'s request.
    pub async fn accept_friend(&self, me: u32, other: u32) -> Result<std::result::Result<(), FriendError>> {
        match self.relation(me, other).await? {
            Relation::Friend => Ok(Ok(())),
            Relation::RequestReceived => {
                self.make_friends(me, other).await?;
                self.push_friend_event(other, me, FriendEventKind::Accepted).await?;
                Ok(Ok(()))
            }
            _ => Ok(Err(FriendError::NoRequest)),
        }
    }

    /// Makes two players friends, replacing any request between them
    /// (never a block: a blocked pair stays blocked).
    pub async fn make_friends(&self, a: u32, b: u32) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let blocked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM relationships WHERE kind = 'block' AND ((user_id = ? AND other_id = ?) OR (user_id = ? AND other_id = ?))",
        )
        .bind(a)
        .bind(b)
        .bind(b)
        .bind(a)
        .fetch_one(&mut *tx)
        .await?;
        if blocked > 0 || a == b {
            return Ok(false);
        }
        for (x, y) in [(a, b), (b, a)] {
            sqlx::query("INSERT INTO relationships (user_id, other_id, kind) VALUES (?, ?, 'friend') ON CONFLICT(user_id, other_id) DO UPDATE SET kind = 'friend', created_at = CURRENT_TIMESTAMP")
                .bind(x)
                .bind(y)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Ends a friendship, or a request either way, between `me` and
    /// `other`. Blocks stay. Returns whether they were friends.
    pub async fn remove_friend(&self, me: u32, other: u32) -> Result<bool> {
        let was_friend = self.relation(me, other).await? == Relation::Friend;
        sqlx::query("DELETE FROM relationships WHERE kind IN ('friend', 'request') AND ((user_id = ? AND other_id = ?) OR (user_id = ? AND other_id = ?))")
            .bind(me)
            .bind(other)
            .bind(other)
            .bind(me)
            .execute(&self.pool)
            .await?;
        Ok(was_friend)
    }

    /// `me` blocks `other`: everything else between them goes (friendship,
    /// requests, invitations, notifications), and `other` stops seeing `me`.
    pub async fn block(&self, me: u32, other: u32) -> Result<std::result::Result<(), FriendError>> {
        if me == other {
            return Ok(Err(FriendError::Yourself));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM relationships WHERE kind IN ('friend', 'request') AND ((user_id = ? AND other_id = ?) OR (user_id = ? AND other_id = ?))")
            .bind(me)
            .bind(other)
            .bind(other)
            .bind(me)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO relationships (user_id, other_id, kind) VALUES (?, ?, 'block') ON CONFLICT(user_id, other_id) DO UPDATE SET kind = 'block'")
            .bind(me)
            .bind(other)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM invites WHERE (sender = ? AND receiver = ?) OR (sender = ? AND receiver = ?)")
            .bind(me)
            .bind(other)
            .bind(other)
            .bind(me)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM friend_events WHERE (user_id = ? AND other_id = ?) OR (user_id = ? AND other_id = ?)")
            .bind(me)
            .bind(other)
            .bind(other)
            .bind(me)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Ok(()))
    }

    /// `me` unblocks `other`. Returns whether they were blocked.
    pub async fn unblock(&self, me: u32, other: u32) -> Result<bool> {
        let done = sqlx::query("DELETE FROM relationships WHERE user_id = ? AND other_id = ? AND kind = 'block'")
            .bind(me)
            .bind(other)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Queues something to tell `user` about `other` (one of each kind per pair).
    pub async fn push_friend_event(&self, user: u32, other: u32, kind: FriendEventKind) -> Result<()> {
        sqlx::query(
            "INSERT INTO friend_events (user_id, other_id, kind) SELECT ?, ?, ?
             WHERE NOT EXISTS (SELECT 1 FROM friend_events WHERE user_id = ? AND other_id = ? AND kind = ?)",
        )
        .bind(user)
        .bind(other)
        .bind(kind.as_str())
        .bind(user)
        .bind(other)
        .bind(kind.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The oldest thing to tell `user`, removed as it's handed out.
    pub async fn take_friend_event(&self, user: u32) -> Result<Option<(FriendEventKind, Person)>> {
        let mut tx = self.pool.begin().await?;
        let row: Option<(i64, u32, String)> = sqlx::query_as("SELECT id, other_id, kind FROM friend_events WHERE user_id = ? ORDER BY id LIMIT 1")
            .bind(user)
            .fetch_optional(&mut *tx)
            .await?;
        let Some((id, other, kind)) = row else {
            return Ok(None);
        };
        sqlx::query("DELETE FROM friend_events WHERE id = ?").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        let kind = if kind == "accepted" { FriendEventKind::Accepted } else { FriendEventKind::Request };
        Ok(self.find_person(other).await?.map(|p| (kind, p)))
    }

    // ---------- Identity (see federation.rs) ----------

    /// Links `user` to their cross-server identity (already verified). A key
    /// already linked to another account here is refused.
    pub async fn link_global_id(&self, user: u32, global_id: &str) -> Result<()> {
        if let Some(owner) = self.find_person_by_global_id(global_id).await? {
            if owner.id != user {
                return Err(eyre!("that identity is already linked to {} on this server", owner.username));
            }
            return Ok(());
        }
        sqlx::query("UPDATE users SET global_id = ? WHERE id = ?").bind(global_id).bind(user).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn find_person_by_global_id(&self, global_id: &str) -> Result<Option<Person>> {
        Ok(sqlx::query_as(&format!("SELECT {PERSON} FROM users u WHERE u.global_id = ?"))
            .bind(global_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Records a sign-in signed at `time` (Unix seconds) and says whether
    /// it's newer than the last one: each signature works once.
    pub async fn use_key_login(&self, user: u32, time: i64) -> Result<bool> {
        let done = sqlx::query("UPDATE users SET last_key_login = ? WHERE id = ? AND (last_key_login IS NULL OR last_key_login < ?)")
            .bind(time)
            .bind(user)
            .bind(time)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Replaces `user`'s password.
    pub async fn set_password(&self, user: u32, password: &str) -> Result<()> {
        let password = password.to_owned();
        let hash = super::hashing(move || {
            use argon2::password_hash::rand_core::OsRng;
            use argon2::password_hash::SaltString;
            use argon2::PasswordHasher;
            let salt = SaltString::try_from_rng(&mut OsRng).unwrap();
            argon2::Argon2::default().hash_password(password.as_bytes(), salt.as_salt()).map(|h| h.to_string())
        })
        .await?
        .map_err(|_| eyre!("password hashing failed"))?;
        sqlx::query("UPDATE users SET password = NULL, password_hash = ? WHERE id = ?").bind(hash).bind(user).execute(&self.pool).await?;
        Ok(())
    }

    // ---------- Federation outbox ----------

    pub async fn outbox_push(&self, body: &str) -> Result<()> {
        sqlx::query("INSERT INTO federation_outbox (body) VALUES (?)").bind(body).execute(&self.pool).await?;
        Ok(())
    }

    /// The oldest waiting messages, at most `limit`.
    pub async fn outbox_peek(&self, limit: u32) -> Result<Vec<(i64, String)>> {
        Ok(sqlx::query_as("SELECT id, body FROM federation_outbox ORDER BY id LIMIT ?").bind(limit).fetch_all(&self.pool).await?)
    }

    pub async fn outbox_remove(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM federation_outbox WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    /// How many players are online, and how many accounts there are.
    pub async fn player_counts(&self) -> Result<(u32, u32)> {
        let (online, total): (i64, i64) = sqlx::query_as(&format!(
            "SELECT COALESCE(SUM(u.is_online), 0), COUNT(*) FROM users u WHERE {}",
            Self::players()
        ))
        .fetch_one(&self.pool)
        .await?;
        Ok((u32::try_from(online).unwrap_or(0), u32::try_from(total).unwrap_or(0)))
    }

    /// Online players linked to a cross-server identity: whose friends to
    /// sync now and then.
    pub async fn online_linked(&self) -> Result<Vec<Person>> {
        Ok(sqlx::query_as(&format!("SELECT {PERSON} FROM users u WHERE u.is_online = 1 AND u.global_id IS NOT NULL"))
            .fetch_all(&self.pool)
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::temp_storage;
    use crate::storage::run;

    fn user(storage: &Storage, name: &str) -> u32 {
        storage.register_user(name, "password1", Some(name)).unwrap();
        storage.find_user_id_by_name(name).unwrap().unwrap()
    }

    #[test]
    fn request_accept_remove() {
        let (s, dir) = temp_storage("friends-flow");
        let (kiwi, tank) = (user(&s, "Kiwi"), user(&s, "Tank"));
        assert_eq!(run(s.request_friend(kiwi, tank)).unwrap().unwrap(), Ok(Relation::RequestSent));
        assert_eq!(run(s.relation(tank, kiwi)).unwrap().unwrap(), Relation::RequestReceived);
        let (kind, from) = run(s.take_friend_event(tank)).unwrap().unwrap().unwrap();
        assert_eq!((kind, from.username.as_str()), (FriendEventKind::Request, "Kiwi"));
        assert!(run(s.take_friend_event(tank)).unwrap().unwrap().is_none(), "handed out once");

        assert_eq!(run(s.accept_friend(tank, kiwi)).unwrap().unwrap(), Ok(()));
        assert_eq!(run(s.relation(kiwi, tank)).unwrap().unwrap(), Relation::Friend);
        assert_eq!(run(s.friends_of(kiwi)).unwrap().unwrap().len(), 1);
        let (kind, _) = run(s.take_friend_event(kiwi)).unwrap().unwrap().unwrap();
        assert_eq!(kind, FriendEventKind::Accepted);

        assert!(run(s.remove_friend(kiwi, tank)).unwrap().unwrap());
        assert_eq!(run(s.relation(tank, kiwi)).unwrap().unwrap(), Relation::None);
        assert!(run(s.friends_of(tank)).unwrap().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn asking_back_makes_friends() {
        let (s, dir) = temp_storage("friends-cross");
        let (a, b) = (user(&s, "A1"), user(&s, "B1"));
        run(s.request_friend(a, b)).unwrap().unwrap().unwrap();
        assert_eq!(run(s.request_friend(b, a)).unwrap().unwrap(), Ok(Relation::Friend));
        assert_eq!(run(s.accept_friend(a, a)).unwrap().unwrap(), Err(FriendError::NoRequest));
        assert_eq!(run(s.request_friend(a, a)).unwrap().unwrap(), Err(FriendError::Yourself));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn blocking_hides_both_ways_and_ends_everything() {
        let (s, dir) = temp_storage("friends-block");
        let (me, pest, other) = (user(&s, "Me"), user(&s, "Pest"), user(&s, "Other"));
        run(s.request_friend(pest, me)).unwrap().unwrap().unwrap();
        run(s.accept_friend(me, pest)).unwrap().unwrap().unwrap();
        run(s.add_invite_async(pest, me, None, None)).unwrap().unwrap();

        run(s.block(me, pest)).unwrap().unwrap().unwrap();
        assert_eq!(run(s.relation(me, pest)).unwrap().unwrap(), Relation::Blocked);
        assert_eq!(run(s.relation(pest, me)).unwrap().unwrap(), Relation::BlockedBy);
        assert!(run(s.friends_of(me)).unwrap().unwrap().is_empty());
        assert!(run(s.take_invite_async(me)).unwrap().unwrap().is_none(), "their invite went");
        let names = |v: Vec<Person>| v.into_iter().map(|p| p.username).collect::<Vec<_>>();
        assert_eq!(names(run(s.everyone_for(pest)).unwrap().unwrap()), ["Other"], "I'm hidden from them");
        assert_eq!(names(run(s.everyone_for(me)).unwrap().unwrap()), ["Other"]);
        assert!(run(s.search_players(pest, "me", 10)).unwrap().unwrap().is_empty());

        // Their request looks sent but goes nowhere; mine is refused until I unblock.
        assert_eq!(run(s.request_friend(pest, me)).unwrap().unwrap(), Ok(Relation::RequestSent));
        assert!(run(s.friend_requests(me, true)).unwrap().unwrap().is_empty());
        assert_eq!(run(s.request_friend(me, pest)).unwrap().unwrap(), Err(FriendError::YouBlocked));
        assert!(!run(s.make_friends(me, pest)).unwrap().unwrap(), "sync can't override a block");

        assert!(run(s.unblock(me, pest)).unwrap().unwrap());
        assert_eq!(run(s.relation(me, pest)).unwrap().unwrap(), Relation::None);
        let _ = other;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn search_by_part_of_a_name_any_case() {
        let (s, dir) = temp_storage("friends-search");
        let me = user(&s, "Searcher");
        for n in ["Kiwi", "kiwifruit_x", "Tank", "BigKiwi"] {
            user(&s, n);
        }
        let found: Vec<String> = run(s.search_players(me, "KIWI", 10)).unwrap().unwrap().into_iter().map(|(p, _)| p.username).collect();
        assert_eq!(found, ["Kiwi", "kiwifruit_x", "BigKiwi"], "names that start with it first");
        assert_eq!(run(s.search_players(me, "%", 10)).unwrap().unwrap().len(), 0, "wildcards are literal");
        assert_eq!(run(s.find_person_by_name("tank")).unwrap().unwrap().unwrap().username, "Tank");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_are_unique_whatever_the_case() {
        let (s, dir) = temp_storage("friends-case");
        user(&s, "Kiwi");
        assert!(s.register_user("KIWI", "password1", Some("KIWI")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_an_account_with_invites_and_friends_works() {
        let (s, dir) = temp_storage("friends-delete");
        let (a, b) = (user(&s, "Gone"), user(&s, "Stays"));
        run(s.request_friend(a, b)).unwrap().unwrap().unwrap();
        run(s.add_invite_async(a, b, None, None)).unwrap().unwrap();
        run(s.add_invite_async(b, a, None, None)).unwrap().unwrap();
        run(s.delete_user_async(a)).unwrap().unwrap();
        assert!(run(s.find_person(a)).unwrap().unwrap().is_none());
        assert!(run(s.take_invite_async(b)).unwrap().unwrap().is_none());
        assert!(run(s.friend_requests(b, true)).unwrap().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invites_queue_one_per_sender() {
        let (s, dir) = temp_storage("friends-invites");
        let me = user(&s, "Host0");
        let senders: Vec<u32> = (1..=6).map(|i| user(&s, &format!("Sender{i}"))).collect();
        for &from in &senders[..5] {
            run(s.add_invite_async(from, me, None, None)).unwrap().unwrap();
        }
        assert!(run(s.add_invite_async(senders[5], me, None, None)).unwrap().is_err(), "at most five waiting");
        run(s.add_invite_async(senders[0], me, None, None)).unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(i) = run(s.take_invite_async(me)).unwrap().unwrap() {
            got.push(i.sender);
        }
        got.sort_unstable();
        assert_eq!(got, senders[..5], "nobody's invite pushed out another's");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn key_logins_work_once() {
        let (s, dir) = temp_storage("friends-keylogin");
        let me = user(&s, "Keyed");
        assert!(run(s.use_key_login(me, 100)).unwrap().unwrap());
        assert!(!run(s.use_key_login(me, 100)).unwrap().unwrap(), "replayed");
        assert!(!run(s.use_key_login(me, 99)).unwrap().unwrap(), "older");
        assert!(run(s.use_key_login(me, 101)).unwrap().unwrap());
        run(s.link_global_id(me, "GID")).unwrap().unwrap();
        let other = user(&s, "Other2");
        assert!(run(s.link_global_id(other, "GID")).unwrap().is_err(), "one account per identity");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
