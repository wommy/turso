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

## All five exist on the fork

Verified present. `wontfix` came from upstream Turso; the other four were
created for these skills:

| Label | Colour | Description |
| --- | --- | --- |
| `needs-triage` | `fbca04` | Maintainer needs to evaluate this issue |
| `needs-info` | `d876e3` | Waiting on reporter for more information |
| `ready-for-agent` | `0e8a16` | Fully specified, ready for an AFK agent |
| `ready-for-human` | `1d76db` | Requires human implementation |
| `wontfix` | `ffffff` | This will not be worked on (upstream's wording) |

To recreate them on another fork, from a machine with `gh` — there is no
label-creation tool in the GitHub MCP server, so this cannot be done from a
remote container:

```bash
gh label create needs-triage    --repo <owner>/turso --color fbca04 --description "Maintainer needs to evaluate this issue"
gh label create needs-info      --repo <owner>/turso --color d876e3 --description "Waiting on reporter for more information"
gh label create ready-for-agent --repo <owner>/turso --color 0e8a16 --description "Fully specified, ready for an AFK agent"
gh label create ready-for-human --repo <owner>/turso --color 1d76db --description "Requires human implementation"
```

`wontfix` ships with most GitHub repos already; `gh label create` errors on a
duplicate, so pass `--force` to overwrite instead.

Edit the middle column of the role table above if the vocabulary ever changes;
the skills read the label string from there, not from the role name.
