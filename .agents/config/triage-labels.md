# Triage labels

The skills speak in terms of five canonical triage roles. This file maps those
roles to the label strings actually used in this repo's tracker; the skills read
the middle column, not the role name, so edit that if the vocabulary changes.

| Role | Label in our tracker | Meaning |
| --- | --- | --- |
| `needs-triage` | `needs-triage` | Maintainer needs to evaluate this issue |
| `needs-info` | `needs-info` | Waiting on reporter for more information |
| `ready-for-agent` | `ready-for-agent` | Fully specified, ready for an AFK agent |
| `ready-for-human` | `ready-for-human` | Requires human implementation |
| `wontfix` | `wontfix` | Will not be actioned |

We kept the defaults rather than mapping onto upstream Turso's labels. Upstream's
vocabulary is aimed at a public OSS triage queue; these five are aimed at one
question an agent has to answer — is this ticket ready to be worked unattended —
and nothing upstream uses says that.

All five exist on the fork, so `/triage` is ready to use. `wontfix` came from
upstream Turso; the other four were created for these skills.

## Creating them somewhere else

**There is no label-creation tool in the GitHub MCP server**, and `issue_write`
does not create a missing label — it fails with `failed to resolve label`. So
labels have to be created ahead of time, from a machine that has `gh`:

```bash
gh label create needs-triage    --repo <owner>/turso --color fbca04 --description "Maintainer needs to evaluate this issue"
gh label create needs-info      --repo <owner>/turso --color d876e3 --description "Waiting on reporter for more information"
gh label create ready-for-agent --repo <owner>/turso --color 0e8a16 --description "Fully specified, ready for an AFK agent"
gh label create ready-for-human --repo <owner>/turso --color 1d76db --description "Requires human implementation"
```

`wontfix` ships with most GitHub repos already, and `gh label create` errors on a
duplicate — pass `--force` to overwrite instead.

This is also why the `wayfinder:*` type labels do not exist: nobody with `gh` has
created them, so wayfinder tickets record their type in the issue body.
