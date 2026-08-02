A copy of one machine's cgroup v2 memory files, taken as they were.

Everything else the resource tests use is a hierarchy someone wrote by hand,
which means it holds what its author believed `memory.stat` contains. This one
holds what a kernel actually put there: seventy lines per group rather than the
half dozen a hand-written fixture bothers with.

It exists so that the reading of those files is checked against the kernel and
not against an idea of it. The test that uses it requires the breakdown to
account for `memory.current` in every group, so a field the code does not know
about shows up as an unexplained remainder rather than as silently free memory.

Regenerate with `mise run fixtures:cgroup`, which is worth doing on a newer
kernel: what fails afterwards is the model, and that is the point.

Captured on Linux 6.19 with systemd, twenty-four cores. Nothing here is
sensitive - byte counts, unit names, and a numeric user id.
