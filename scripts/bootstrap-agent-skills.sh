#!/usr/bin/env bash
set -euo pipefail

# Reinstall Matt Pocock's engineering skills in a fresh container.
#
# The skills install into ~/.claude/skills and ~/.agents/skills, both outside
# the repo, so they survive no container restart. This restores them in one
# command. See docs/agents/README.md.

CLONE="${AGENT_SKILLS_CLONE:-$HOME/mattpocock-skills}"
REMOTE="https://github.com/mattpocock/skills.git"

if [ -d "$CLONE/.git" ]; then
  echo "updating $CLONE"
  git -C "$CLONE" pull --ff-only
else
  echo "cloning $REMOTE into $CLONE"
  git clone --depth 1 "$REMOTE" "$CLONE"
fi

# link-skills.sh is the repo's own dev installer: it symlinks every skill into
# both harness directories, so a later `git pull` in the clone updates them all.
bash "$CLONE/scripts/link-skills.sh"

# Matt's code-review owns the `code-review` name, which the harness also uses
# for its own built-in review. This alias is the name that unambiguously means
# his two-axis Standards+Spec review, whichever way the collision resolves.
for dest in "$HOME/.claude/skills" "$HOME/.agents/skills"; do
  ln -sfn "$CLONE/skills/engineering/code-review" "$dest/two-axis-review"
done
echo "linked two-axis-review in both harness directories"

echo
echo "Done. Run /setup-matt-pocock-skills only if the repo config is missing;"
echo "it is already committed on this branch under docs/agents/."
