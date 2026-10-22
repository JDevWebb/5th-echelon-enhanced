# Chat

Text chat for the community network, in the launcher and in the game's overlay. Blacklist has no text chat of its own on PC (its lobbies only have voice), so this is ours.

Chat is part of the **community network** only (`scbl.jdevwebb.net` and its servers). A server or network of your own doesn't have it: see [Running your own](#running-your-own).

## Rooms

| Room | Who's in it | Kept |
|---|---|---|
| **Global** | everyone on the community network, on every server | 7 days, the newest 500 |
| **Lobby** | the players in your game session (party or match), on your server | 2 hours |
| **Messages** | you and one friend, whatever servers you're each on | 30 days, the newest 200 a conversation |

- **Global** needs an account linked to your identity (the launcher makes one when you connect), and a new account waits until it has been signed in for 30 minutes or played a match, so a fresh account can't post straight away.
- **Messages** are between friends only, and not when either of you has blocked the other.
- **Lobby** shows while you're in a party or a match.

## Where it is

- **The launcher:** the **Chat** page on the left (on the community network): Global with who's online beside it, Lobby while you're in one, and Messages, with unread counts. The rail counts direct messages and mentions you haven't seen, and the taskbar flashes for them when the launcher is behind other windows.
- **The game:** the overlay's **Chat** tab (<kbd>F5</kbd>, or your overlay key), with the same rooms. New messages pop up at the top right as **Settings › Notifications** says, per room: never, only when you're mentioned, outside matches, or always. By default Global pops up outside matches, and Lobby and your friends always.
- **On each message**, right-click the name: add them as a friend, block them, message them (a friend, in the launcher), report the message, or delete your own. To invite someone, use **Online now** on the Friends card or **Find players** in the overlay.
- **@name** mentions are highlighted for the player named.

## Rules

Chat keeps to the network's rules, checked on your server and again on the coordinator:

- **No links**, except a Discord invite in its own form (`discord.gg/…`), so players can bring each other to a Discord: in Global, one every 10 minutes. The launcher asks before opening one, and says while you type when a link won't be sent.
- **Slurs and hate are masked** (`***`, the whole word), in every room. Swearing is allowed.
- **Not too fast:** 5 messages in 10 seconds, 30 a minute, and the same message not again within 30 seconds.
- Control, zero-width and direction-changing characters are taken out of messages.

Names follow the same line: a name with a slur, hate or a web address is refused (it can't be masked), and players with one from before are asked to choose another.

## Reports and the admins

- **Messages are kept for the times above, and the network's admins can read them.** The launcher says so on the Chat page.
- **Report** a message with a reason. The admins see it on the admin site's **Moderation** page, with the 20 messages before it (in a direct conversation, only the messages next to it), who reported it and why, and the sender's accounts.
- A reported message stays until the admins have dealt with it, whatever happens to the conversation or the sender's account, so the evidence can't be wiped first; a report and its message go after 90 days at most.
- What the admins can do: **delete** the message (it goes from every player's view), **mute** the sender in one room or everywhere (an hour, a day, 7 days or for good), **ban** their accounts, or **dismiss** the report. Mutes, bans and deletes need a recent second factor and go in the audit log. A player's page shows their mutes in force, each with **Lift**. Nothing is muted automatically.
- **Deleting your own message** hides it for everyone.
- **Blocking** someone hides their messages from you everywhere, and stops their direct messages.

Messages refused for a link are kept 7 days for the admins. The admins also see who had the most words masked this week (the count, not the words).

## How it works

- Your launcher and game talk to **your own server**, which speaks for you to the community coordinator. The coordinator keeps every room's messages and hands each server what's for its players.
- A server can only speak for players linked to it, under the name that's theirs across the network, and only a server running the project's signed release can pass messages at all.
- Delivery is a long poll, not a push channel: your server waits on the coordinator for new messages (up to 25 seconds at a time), and your launcher and game wait on your server the same way. Plain HTTP and gRPC, so it works through any proxy.

## Running your own

Your own server or network doesn't get chat: the coordinator runs it only for the community network (`--chat`, which the installer sets only there), and a server on another network says *This server's network has no chat*. Everything else in the game (friends, invites, who's online) works the same.

For a test network built from a branch, `--chat --chat-unverified` on its coordinator lets servers that aren't the signed release pass messages.
