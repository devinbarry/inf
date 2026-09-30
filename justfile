# Default recipe - show available commands
default:
    @just --list

# Step 1: Prepare a release (bump version, generate changelog draft, do NOT commit)
release-prep level:
    #!/usr/bin/env bash
    set -euo pipefail

    # Read the version straight out of Cargo.toml. Parsing `cargo metadata`
    # needed python3, an undeclared dependency in a Rust project.
    OLD_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    if [[ -z "$OLD_VERSION" ]]; then
        echo "Error: could not read version from Cargo.toml"; exit 1
    fi
    echo "Current version: $OLD_VERSION"

    # Calculate new version
    IFS='.' read -r MAJOR MINOR PATCH <<< "$OLD_VERSION"
    case "{{level}}" in
        patch) PATCH=$((PATCH + 1)) ;;
        minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
        major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
        *) echo "Usage: just release-prep [patch|minor|major]"; exit 1 ;;
    esac
    NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"
    echo "New version: $NEW_VERSION"

    # Bump version in Cargo.toml (compatible with both macOS and Linux sed)
    if [[ "$OSTYPE" == darwin* ]]; then
        sed -i '' "s/^version = \"$OLD_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
    else
        sed -i "s/^version = \"$OLD_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
    fi
    # Update Cargo.lock. Errors are shown: suppressing them left a half-bumped
    # Cargo.toml and no explanation of why the recipe stopped.
    cargo check --quiet

    # Generate changelog draft to stdout for reference
    echo ""
    echo "=== git-cliff draft for v${NEW_VERSION} (unreleased commits) ==="
    echo ""
    git-cliff --unreleased --tag "$NEW_VERSION" 2>/dev/null || echo "(git-cliff not available, write changelog manually)"
    echo ""
    echo "=== end draft ==="
    echo ""
    echo "Next steps:"
    echo "  1. Edit CHANGELOG.md with release notes for v${NEW_VERSION}"
    echo "  2. Review all changes: git diff"
    echo "  3. Run: just release-finish"

# Step 2: Commit, tag, push, and install (run after reviewing release-prep changes)
release-finish:
    #!/usr/bin/env bash
    set -euo pipefail

    VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    echo "Releasing v${VERSION}"

    # Check what the old guard only claimed to check. `release-prep` always
    # dirties Cargo.toml, so "are there changes?" was satisfied by the version
    # bump alone and would happily tag an unedited changelog.
    if ! grep -q "^## \[${VERSION}\]" CHANGELOG.md; then
        echo "Error: CHANGELOG.md has no '## [${VERSION}]' section."
        echo "Run 'just release-prep' and write the release notes first."
        exit 1
    fi

    # Fail before creating a tag that cannot be pushed.
    if ! git remote get-url origin >/dev/null 2>&1; then
        echo "Error: no 'origin' remote configured; nothing to push to."
        exit 1
    fi

    # Refuse to release something that does not pass its own checks.
    just check

    # Refuse to tag a version that is already tagged.
    if git rev-parse -q --verify "refs/tags/v${VERSION}" >/dev/null; then
        echo "Error: tag v${VERSION} already exists. Bump the version first."
        exit 1
    fi

    # Commit, tag, push, install. The release commit is skipped when there is
    # nothing to put in it — the case for an initial release, where the version
    # in Cargo.toml is already correct and `release-prep` never ran.
    git add Cargo.toml Cargo.lock CHANGELOG.md
    if git diff --cached --quiet; then
        echo "No release-only changes to commit; tagging the current HEAD."
    else
        git commit -m "chore: release v${VERSION}"
    fi
    git tag -a "v${VERSION}" -m "v${VERSION}"
    git push && git push --tags
    cargo install --locked --path . --force
    echo ""
    echo "Released v${VERSION}"

# Run tests
test:
    cargo test

# Run clippy
lint:
    cargo clippy --all-targets -- -D warnings

# Format code
fmt:
    cargo fmt

# Run all CI checks locally (fmt, clippy, test)
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

# Install binary to ~/.cargo/bin (cargo default)
install:
    cargo install --locked --path . --force
