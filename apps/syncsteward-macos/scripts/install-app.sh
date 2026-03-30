#!/bin/zsh
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
package_path="$repo_root/apps/syncsteward-macos"
bin_dir="$(swift build --package-path "$package_path" --show-bin-path)"
ui_bin="$bin_dir/syncsteward-macos"
cli_bin="$repo_root/target/debug/syncsteward-cli"
bundle_root="$HOME/Applications/SyncSteward.app"
contents_dir="$bundle_root/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
plist_source="$package_path/Bundle/Info.plist"
launcher_path="$macos_dir/SyncSteward"

swift build --package-path "$package_path" >/dev/null
cargo build -p syncsteward-cli >/dev/null

mkdir -p "$macos_dir" "$resources_dir"
cp "$plist_source" "$contents_dir/Info.plist"

cat >"$launcher_path" <<EOF
#!/bin/zsh
set -euo pipefail
export SYNCSTEWARD_CLI_PATH="$cli_bin"
exec "$ui_bin"
EOF

chmod +x "$launcher_path"

echo "Installed $bundle_root"
