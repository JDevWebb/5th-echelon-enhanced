#!/bin/sh
# rc.exe stand-in for winres when cross-compiling from Linux: winres runs
# "<toolkit>/rc.exe /I<dir> /fo<out> <in.rc>" and, off Windows, its toolkit
# path is "/", so this is installed as /rc.exe and forwards to llvm-rc.
set -eu
for a in "$@"; do
  case "$a" in
    /I*) set -- "$@" -I "${a#/I}" ;;
    /fo*) set -- "$@" -FO "${a#/fo}" ;;
    *) set -- "$@" "$a" ;;
  esac
  shift
done
exec llvm-rc "$@"
