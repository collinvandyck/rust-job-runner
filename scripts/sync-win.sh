#!/usr/bin/env bash

set -eu

LABEL=name=job-runner-sync

start() {
    mutagen sync terminate --all
    mutagen sync create \
        --configuration-file .mutagen.yml \
        --ignore-vcs \
        --name win-sync \
        --label "${LABEL}" \
        "$(git rev-parse --show-toplevel)" \
        collin@${HOST}:~/code/job-runner-sync
    mutagen sync monitor "${LABEL}" --long
}

stop() {
    mutagen sync terminate --label-selector "${LABEL}"
}

monitor() {
    mutagen sync monitor --label-selector "${LABEL}"
}

list() {
    mutagen sync list --label-selector "${LABEL}" --long
}


[[ $# = 0 ]] && {
    echo "usage: $(basename "$0") start|stop|monitor"
    exit 1
}

case "$1" in
    start)
        start
        ;;
    stop)
        stop
        ;;
    monitor)
        monitor
        ;;
    list)
        list
        ;;
    *)
        echo "unknown command: {$1}"
        exit 1
esac

