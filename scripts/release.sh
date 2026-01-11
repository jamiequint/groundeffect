#!/bin/bash
set -euo pipefail

# Get version from Cargo.toml
VERSION=$(grep -m1 'version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
echo "Building release v$VERSION"

# Detect architecture
ARCH=$(uname -m)
if [ "$ARCH" = "arm64" ]; then
    TARGET_ARCH="darwin-arm64"
elif [ "$ARCH" = "x86_64" ]; then
    TARGET_ARCH="darwin-x86_64"
else
    echo "Unsupported architecture: $ARCH"
    exit 1
fi

# Detect OS and set features
OS=$(uname -s)
if [ "$OS" = "Darwin" ]; then
    FEATURES="metal"
    echo "Detected macOS - enabling Metal GPU acceleration"
elif [ "$OS" = "Linux" ]; then
    # Check if CUDA is available
    if command -v nvidia-smi &> /dev/null; then
        FEATURES="cuda"
        echo "Detected Linux with NVIDIA GPU - enabling CUDA acceleration"
    else
        FEATURES=""
        echo "Detected Linux without NVIDIA GPU - using CPU"
    fi
else
    FEATURES=""
    echo "Unknown OS - using CPU"
fi

# Build release binaries
echo "Building release binaries..."
if [ -n "$FEATURES" ]; then
    cargo build --release --features "$FEATURES"
else
    cargo build --release
fi

# Create dist directory
DIST_DIR="dist"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

# Copy binaries
echo "Copying binaries..."
cp target/release/groundeffect "$DIST_DIR/"
cp target/release/groundeffect-daemon "$DIST_DIR/"
cp target/release/groundeffect-mcp "$DIST_DIR/"

# Copy skill files
echo "Copying skill files..."
mkdir -p "$DIST_DIR/skill"
cp -r skill/* "$DIST_DIR/skill/"

# Create tarball
TARBALL="groundeffect-$VERSION-$TARGET_ARCH.tar.gz"
echo "Creating $TARBALL..."
cd "$DIST_DIR"
tar -czvf "../$TARBALL" groundeffect groundeffect-daemon groundeffect-mcp skill/
cd ..

# Calculate SHA256
SHA256=$(shasum -a 256 "$TARBALL" | cut -d' ' -f1)

echo ""
echo "=== Release v$VERSION ==="
echo "Tarball: $TARBALL"
echo "SHA256: $SHA256"
echo ""
echo "To create the release:"
echo "  1. git tag v$VERSION && git push origin v$VERSION"
echo "  2. gh release create v$VERSION $TARBALL --title \"v$VERSION\" --notes \"Release v$VERSION\""
echo "  3. Update homebrew-groundeffect/Formula/groundeffect.rb with:"
echo "     - version \"$VERSION\""
echo "     - sha256 \"$SHA256\""
