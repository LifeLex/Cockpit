DEST="/Users/alejandroperez/Code/Personal/10-cockpit"
DL="/Users/alejandroperez/Downloads"

# unpack the scaffold here, flattening the top-level cockpit/ dir
tar -xzf cockpit-scaffold.tar.gz --strip-components=1
rm cockpit-scaffold.tar.gz # so it isn't committed

# move the three docs in (adjust path if they're not in Downloads)
mv "$DL/SPEC.md" "$DL/CLAUDE.md" "$DL/IMPLEMENTATION_PLAN.md" "$DEST/"

# sanity check
ls -a
ls .claude/agents/

# verify T0.1 (run `rustup update` if it errors on edition 2024)
cargo build && cargo test
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings

# baseline commit on main (local only)
git init -b main
git add -A
git commit -m "chore: scaffold workspace, docs, and subagents (T0.1)"

# start the loop at the plan gate for T0.2
git switch -c alejandro/t0.2-domain-model
