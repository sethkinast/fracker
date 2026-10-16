#!/bin/sh
# Flatpak launcher for Fracker (sixth pass §3, addendum #30).
#
# Pins the three runtime seams the OCR engine probes and the data loader
# need, then execs the real binary:
#   - FRACKER_DATA_DIR  : the bundled data/ tree (read-only /app share)
#   - TESSDATA_PREFIX   : the bundled eng tessdata
#   - FRACKER_TESSERACT : the tesseract sidecar (the probe's FIRST
#                         candidate, ahead of $PATH / exe-relative paths)
set -e

export FRACKER_DATA_DIR="${FRACKER_DATA_DIR:-/app/share/fracker/data}"
export TESSDATA_PREFIX="${TESSDATA_PREFIX:-/app/share/tessdata}"
export FRACKER_TESSERACT="${FRACKER_TESSERACT:-/app/bin/tesseract}"

exec /app/bin/fracker.bin "$@"
