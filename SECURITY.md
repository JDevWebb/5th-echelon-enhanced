# Security policy

Thank you for helping keep 5th Echelon Enhanced and its players safe. If you find a weakness, please tell us privately first, and give us a chance to fix it before anyone else hears about it.

## Reporting a vulnerability

**Please don't open a public issue, discussion or pull request for a security problem.**

Report it privately on GitHub instead: on this repository's **Security** tab, choose **[Report a vulnerability](https://github.com/JDevWebb/5th-echelon-enhanced/security/advisories/new)**. Only the maintainer sees it, and we can talk it through there and credit you in the advisory.

Helpful to include:
- what's affected (the launcher, the client DLL, the server, the coordinator, the admin UI, the installer, or a community server) and which version (Settings › About in the launcher; `/api/info` on a server);
- the steps to reproduce it, and the smallest proof of concept you can;
- what an attacker could do with it, and anything you know about who's exposed;
- whether you'd like credit, and under what name.

## What happens next

- **Within 7 days:** we acknowledge your report.
- **Within 14 days:** we tell you whether we've confirmed it, and how serious we think it is.
- **Then:** we work on a fix and keep you posted. Serious problems that affect the community servers or players' accounts and PCs come first.
- **Disclosure:** we publish an advisory when the fix is released, crediting you unless you'd rather not be named. We aim to fix within 90 days; if that isn't possible, we'll agree a timeline with you. Please don't disclose before then without talking to us.

5th Echelon Enhanced is a free project run by one person, with no bug bounty. What we can offer is a prompt, honest response and credit for your work.

## What's in scope

- **This repository's code:** the launcher, the client DLL (`uplay_r1_loader.dll`), the dedicated server, the coordinator and its admin UI, the Linux installer and `harden-host.sh`, and the release signing and update path.
- **The community network:** `play.scbl.jdevwebb.net`, its game servers (`eu1`, `na1` and `oceania.scbl.jdevwebb.net`) and its admin UI, `scbl-metrics.jdevwebb.net`.
- **Especially interesting:** anything that lets someone take over another player's account or identity, run code on a player's PC or a server, get around the release signature checks, read or change another player's data, use the servers to reach or attack other hosts (the relay, the NAT helper), join the community network without its join token, or get into the admin UI.

## Out of scope

- *Tom Clancy's Splinter Cell: Blacklist* itself, and Ubisoft's services. Report those to Ubisoft.
- Problems that exist only in [upstream 5th Echelon](https://github.com/unixoide/5th-echelon), in code this project doesn't change. Please report those upstream; if they affect this project too, we'd like to hear as well.
- Servers run by other people with this software: contact their operators. If the problem is in this software, report it here.
- Reports with no security impact, such as missing headers with no exploit, version disclosure, or results from an automated scanner without a demonstrated issue.
- Antivirus programs flagging the launcher or the client DLL: see [Antivirus warnings](README.md#antivirus-warnings).

## Testing the community servers in good faith

We won't pursue or complain about research that follows these rules, and we'll treat it as authorised:
- **Use your own accounts,** and only touch data that's yours. Don't access, change or delete other players' accounts, friends, messages or data; if you come across any, stop and tell us.
- **Don't disrupt the service:** no denial-of-service or volumetric testing, flooding, spam, or tests that degrade play for others. A local server (`build/build.sh bots`, or a server on your own machine) is the right place for anything heavy; every component runs locally.
- **Keep it minimal:** use the smallest proof of concept that shows the problem, and don't keep any data you obtain.
- **No social engineering** of players or the maintainer, and no physical attacks or attacks on hosting providers.
- **Report it promptly,** and keep it private until it's fixed.

If you're unsure whether something is OK, ask in your private report first.

## Supported versions

Security fixes go into the latest release, and the community network's servers update themselves to it. The launcher updates itself from signed releases (release builds), and servers refuse launchers and clients older than they allow, so older versions aren't patched separately. If you run your own server, keep it updated (see [Updating](docs/deploying.md#updating)).

## What's already in place

The design choices are described in the [README](README.md#whats-new-since-5th-echelon) (Security) and the docs: [friends.md](docs/friends.md) (identities and what the coordinator trusts), [operations.md](docs/operations.md) (updates and the admin UI), [deploying.md](docs/deploying.md) (hardening the machine) and [server-settings.md](docs/server-settings.md).
