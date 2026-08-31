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

## These labels do not exist yet

**`/triage` will fail on first use until they are created.** They were not
created here because the GitHub MCP server has no label-creation tool and the
remote container has no `gh`. Create them from a machine that does:

```bash
for l in needs-triage needs-info ready-for-agent ready-for-human wontfix; do
  gh label create "$l" --repo wommy/turso
done
```

`gh label create` fails on a label that already exists, which is harmless — add
`|| true` if re-running.

Edit the middle column if the vocabulary ever changes; the skills read the
label string from there, not from the role name.
