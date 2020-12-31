# Andrew's Git Utilities

This repo contains various tools that I wrote for personal use on my git
repos. I will make half an effort to make them generally usable, and PRs
are welcome to improve things, but because these are for my own use I tend
to hardcode things and you should definitely skim the source before using
anything that you find here.

## `label-pr`

This is a tool which analyzes PR branches and attaches labels to them via
git notes, so that when you find a commit using `blame` or whatever you
have a readily available link to the PR that brought it in. To use it,
modify your global `.gitconfig` to add

```
[notes]
	displayRef = "refs/notes/label-pr"
```

so that `git log` will show the attached labels, and make sure you have

```
[remote "origin"]
	fetch = +refs/pull/*:refs/remotes/pr/*
	fetch = +refs/merge-requests/*:refs/remotes/pr/*
```
so that `git fetch` will bring in PR branches. (The first line is for
Github, the second for Gitlab.)

Then to use the tool, just compile it with `cargo build --release` and
run it with an invocation like
```
/path/to/target/release/label-pr pr:master:https://github.com/bitcoin/bitcoin/pull/
```
where `pr` is the same as `/pr/` from your `.gitconfig`, `master` indicates
the branch to consider PRs to have branched from (you can add multiple by
comma-separating them, e.g. `master,release-1.0,release-2.0` etc), and the
URL will be prefixed with the PR number in the notes.

You can add as many of these `ref:branch:url` triplets as you want, e.g. if
you are maintaining a fork and have PRs from multiple repos.

