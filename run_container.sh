#!/bin/bash

nohup podman run -d \
-v /home/john/repostrusty/db:/repostrusty/db \
-v /home/john/repostrusty/temp:/repostrusty/temp \
-v /home/john/repostrusty/cookies:/repostrusty/cookies \
repostrusty > /home/john/repostrusty/repostrusty.log 2>&1 &

echo "Container started"