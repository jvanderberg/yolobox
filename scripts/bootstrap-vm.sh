#!/usr/bin/env bash
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive

apt_packages=(
  build-essential
  ca-certificates
  curl
  gcc
  git
  pkg-config
  libssl-dev
  libsqlite3-dev
  make
  unzip
  zip
  python3-pip
  python3-venv
  python3-dev
  pipx
)

sudo apt-get update
sudo apt-get install -y "${apt_packages[@]}"

if ! command -v rustup >/dev/null 2>&1; then
  curl https://sh.rustup.rs -sSf | sh -s -- -y --profile default
fi

source "$HOME/.cargo/env"
rustup toolchain install stable
rustup default stable
rustup component add rustfmt clippy

export NVM_DIR="$HOME/.nvm"
if [ ! -s "$NVM_DIR/nvm.sh" ]; then
  mkdir -p "$NVM_DIR"
  curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash
fi

# shellcheck disable=SC1090
source "$NVM_DIR/nvm.sh"
nvm install --lts
nvm alias default 'lts/*'
set +u
nvm use default
set -u

npm install -g npm@latest
npm install -g vite create-vite @openai/codex @anthropic-ai/claude-code
corepack enable

python3 -m pip install --user --upgrade pip
pipx ensurepath

if ! grep -q 'cargo/env' "$HOME/.bashrc" 2>/dev/null; then
  cat >>"$HOME/.bashrc" <<'EOF'
if [ -f "$HOME/.cargo/env" ]; then
  . "$HOME/.cargo/env"
fi
EOF
fi

if ! grep -q 'nvm use --silent default' "$HOME/.bashrc" 2>/dev/null; then
  cat >>"$HOME/.bashrc" <<'EOF'
export NVM_DIR="$HOME/.nvm"
if [ -s "$NVM_DIR/nvm.sh" ]; then
  . "$NVM_DIR/nvm.sh"
  nvm use --silent default >/dev/null 2>&1 || true
fi
EOF
fi

if ! grep -q 'nvm use --silent default' "$HOME/.profile" 2>/dev/null; then
  cat >>"$HOME/.profile" <<'EOF'
export NVM_DIR="$HOME/.nvm"
if [ -s "$NVM_DIR/nvm.sh" ]; then
  . "$NVM_DIR/nvm.sh"
  nvm use --silent default >/dev/null 2>&1 || true
fi
EOF
fi

cat <<'EOF'
Bootstrap complete.

Installed:
- Rust stable via rustup
- Node LTS via nvm
- npm latest
- vite and create-vite globally
- OpenAI Codex CLI globally
- Claude Code globally
- python3-pip, python3-venv, pipx
- common native build dependencies
EOF
