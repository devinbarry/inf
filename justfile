# Default recipe - show available commands
default:
    @just --list

# Step 1: Prepare a release (bump version, generate changelog draft, do NOT commit)
release-prep level:
    #!/usr/bin/env bash
    set -euo pipefail

    # Bump version in Cargo.toml (dry-run to get new version, then sed)
    OLD_VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
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
    cargo check --quiet 2>/dev/null  # update Cargo.lock

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

    VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
    echo "Releasing v${VERSION}"

    # Verify there are staged or unstaged changes to commit
    if git diff --quiet && git diff --cached --quiet; then
        echo "Error: no changes to commit. Did you run 'just release-prep' and edit CHANGELOG.md?"
        exit 1
    fi

    # Commit, tag, push, install
    git add Cargo.toml Cargo.lock CHANGELOG.md
    git commit -m "chore: release v${VERSION}"
    git tag -a "v${VERSION}" -m "v${VERSION}"
    git push && git push --tags
    cargo install --path . --force
    echo ""
    echo "Released v${VERSION}"

# Run tests
test:
    cargo test

# Run clippy
lint:
    cargo clippy -- -D warnings

# Format code
fmt:
    cargo fmt

# Run all CI checks locally (fmt, clippy, test)
check:
    cargo fmt --check
    cargo clippy -- -D warnings
    cargo test

# Install binary to ~/.cargo/bin (cargo default)
install:
    cargo install --path . --force
