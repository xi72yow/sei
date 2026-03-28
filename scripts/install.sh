#!/bin/bash
set -euo pipefail

# Install script for the sei APT repository
# Usage: curl -fsSL https://xi72yow.github.io/sei/install.sh | sudo bash

REPO_URL="${REPO_URL:-https://xi72yow.github.io/sei}"

echo "Adding sei APT repository..."

# Download and install the GPG key
curl -fsSL "${REPO_URL}/key.gpg" | gpg --dearmor -o /usr/share/keyrings/sei.gpg

# Add the repository
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/sei.gpg] ${REPO_URL} stable main" \
  > /etc/apt/sources.list.d/sei.list

# Update and install
apt-get update
apt-get install -y sei

echo "sei has been installed successfully!"
echo "Run 'sei' to start the TUI, or 'sei --help' for usage."
