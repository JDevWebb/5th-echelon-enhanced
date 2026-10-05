## What and why

<!-- What changes, for whom, and the problem it solves. Link the issue if there is one. -->

## How it was tested

<!-- What you ran, and what you tried by hand. -->

- [ ] `build/build.sh test` passes, and the code is formatted (`build/build.sh fmt`)
- [ ] Server changes: `build/build.sh bots` passes (with a new `tools/testbot` scenario for new behaviour)
- [ ] Client or launcher changes: tried in the game on Windows (say with how many players), or said below why not
- [ ] Admin UI changes: looked at the pages it touches, in light and dark mode

## Checklist

- [ ] No game files or anything extracted from them
- [ ] Players or admins will notice this: a line in the next release's notes (`docs/releases/`)
- [ ] New data about players leaves their PC or is kept: said plainly where they see it, and in docs/operations.md's privacy section
- [ ] `release.toml`'s version left as it is
