#!/bin/bash

# Get a list of all screen sessions with the name "repostrusty"
sessions=$(screen -ls | grep -o '[0-9]*\.repostrusty')

# Loop over the sessions and terminate each one
for session in $sessions; do
    echo "Removing existing screen session $session..."
    screen -X -S $session quit
done

# Create a new screen session named "repostrusty"
screen -dmS repostrusty

# Run the container inside the screen session
screen -S repostrusty -X stuff "podman run -d \
-v /home/john/repostrusty/temp:/repostrusty/temp \
-v /home/john/repostrusty/config:/repostrusty/config \
-v /home/john/repostrusty/cookies:/repostrusty/cookies \
repostrusty > /home/john/repostrusty/repostrusty.log 2>&1\n"

echo "Container started in a new screen session"