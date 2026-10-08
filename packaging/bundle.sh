#!/bin/sh
# Gather the release builds of every program, and a README, into DIR:
#   packaging/bundle.sh DIR README COMMIT
# README is a text file saying what the bundle is; the commit it was
# built from is appended.  Programs carry .exe where the build made one.
# Run from the top of the tree, after cargo build --release.
set -eu

dir=$1
readme=$2
commit=$3

mkdir -p "$dir"
for program in smartclockd smartclock-cli smartclockmon smartclock-web \
               smartclock-exporter smartclock-sim smartclock-sensord; do
    for built in "target/release/$program" "target/release/$program.exe"; do
        if [ -f "$built" ]; then
            cp "$built" "$dir/"
        fi
    done
done
{ cat "$readme"; echo "$commit"; } > "$dir/README.txt"
