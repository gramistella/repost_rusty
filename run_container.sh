#!/bin/bash

# Check if a screen session with the name "repostrusty" already exists
if screen -list | grep -q "repostrusty"; then
    echo "Removing existing screen session..."
    # Terminate the existing screen session
    screen -X -S repostrusty quit
fi

# Create a new screen session named "repostrusty"
screen -dmS repostrusty

# Run the container inside the screen session
screen -S repostrusty -X stuff "podman run -d \
-v /home/john/repostrusty/db:/repostrusty/db \
-v /home/john/repostrusty/temp:/repostrusty/temp \
-v /home/john/repostrusty/config:/repostrusty/config \
-v /home/john/repostrusty/cookies:/repostrusty/cookies \
repostrusty > /home/john/repostrusty/repostrusty.log 2>&1\n"

echo "Container started in a new screen session"