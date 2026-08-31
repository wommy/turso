# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those
roles to the label strings actually used in this repo's tracker.

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

## Four of these five do not exist yet

`wontfix` already exists on the fork, inherited from upstream Turso (white,
"This will not be worked on"), and its meaning already matches — leave it alone.
The other four need creating, and **`/triage` fails until they are**.

They were not created here because the GitHub MCP server has no label-creation
tool and the remote container has no `gh`. Run this from a machine that does:

```bash
gh label create needs-triage    --repo wommy/turso --color fbca04 --description "Maintainer needs to evaluate this issue"
gh label create needs-info      --repo wommy/turso --color d876e3 --description "Waiting on reporter for more information"
gh label create ready-for-agent --repo wommy/turso --color 0e8a16 --description "Fully specified, ready for an AFK agent"
gh label create ready-for-human --repo wommy/turso --color 1d76db --description "Requires human implementation"
```

`gh label create` errors on a label that already exists; pass `--force` to
overwrite instead. Do not create `wontfix` — it will fail as a duplicate.

Edit the middle column above if the vocabulary ever changes; the skills read the
label string from there, not from the role name.
