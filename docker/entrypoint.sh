#!/bin/bash
echo "[a2ex] Container started at $(date)"
echo "  arch=$(uname -m) os=$(uname -s)"
echo "  disk=$(df -h / | tail -1)"
echo "  node=$(node --version 2>&1)"
echo "  openclaw=$(which openclaw 2>&1)"
echo "  waiaas=$(which waiaas 2>&1)"
echo "  tini=$(which tini 2>&1)"
echo "[a2ex] Keeping container alive for debugging..."
exec sleep infinity
