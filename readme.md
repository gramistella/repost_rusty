# repost_rusty
repost_rusty is a Rust application that selectively reposts reels from a specified set of instagram accounts, leveraging a discord bot for an easy, cross-platform front-end

## The problem
Finding content on instagram is really easy, but reposting it is not as trivial as it sounds. Normally, to accomplish this you would either use a dedicated app (there are tweaked apps, like Rocket for Instagram, that easily allow you to repost content), or you go through the pain and suffering of going to your pc, manually finding the video url and downloading it, then uploading it to instagram, setting up the caption, when it will post, etc. 

While this is a remarkably simple task, it is also a very tedious one, and it is not scalable at all.

## The solution
Just automate the tasks in your favorite language! Right?

Well, to do that, you would need to find a way to scrape reels from instagram, and upload them in an easy to use and high level way.

I searched far and wide for a library that would allow me to do this, and I found a couple of them, but they were either outdated, or didn't work at all.

The only candidate that I found was [instagram-scraper-rs](https://github.com/veeso/instagram-scraper-rs), which while outdated (the last commit was 10/9/2022), seemed simple enough for me to adopt it and update it to my needs.

Initially the crate didn't work at all, it was expecting a field from a json response that was not there, and it just crashed.
Once I got the crate working, I added the following features:
- Permanent cookie storage, to avoid having to log in every time the bot is restarted
- Download reels from a shortcode
- Upload reels from a url
- More robust error handling, including "recoverable" errors, like when the instagram account is restricted and needs to be manually unlocked and "unrecoverable" errors, like when the content you are trying to upload is not recognized as a valid format by instagram.

With these features in place, I was able to move on to the next step.

## The front-end

Initially I wanted to use a telegram bot for this task. I was used to telegram bots as I made a couple of tests with them in the past, and I knew that they were easy to use and to set up.

I picked the Teloxide crate and it worked great, until I realized that it really didn't. For some obscure way (or maybe not so obscure, I didn't really look into it), the chat with the bot would not get completely synchronized with all the devices, leaving dangling buttons that would make the bot crash.

This meant that the bot worked great if you had a device with telegram open all the time, but if you didn't, or if you used multiple devices, the issues above would arise. The user would be then forced to clear the chat and resend the /start message.

In my opinion this was not acceptable, as it kind of defeated the purpose of the bot, which was to make reposting reels as easy as possible. 

I started looking for alternatives that would be cross-platform, easy to use and flexible, and I found the Serenity crate, which is a discord bot framework for Rust.

Using my previous experience with the Teloxide crate, I was able to pretty quickly make the switch and write remarkably cleaner code.

The discord bot is perfect, it is always in sync with all the devices, it is easy to use, and it is cross-platform. It also has a lot of features that I can leverage, like the ability to send embeds, which I use to show the reels that have been reposted, and the reels that have been scraped, and buttons, just like the ones that I used in the telegram bot.

I was briefly plagued by a minor issue regarding the discord bot, which was that the buttons where extremely unreliable. They would work 3 times out of 10, for no apparent reason. 

This made the bot temporarily more annoying than the telegram one, as the user would have to spam the button until it worked, sometime take up to a minute.

Fortunately this issue was a small insidious mistake in the code, and it was eventually fixed. While I can't say that the buttons work 100% of the time, they work a decent 9 times out of 10.

## Dependencies

- instagram-scraper-rs, which is hosted on my github
- [ffmpeg](https://ffmpeg.org/)

## Features

- Multiple accounts management
  - Each one is isolated from the others, and offers the following features
- Content queue, which uses a predefined interval +- a random factor to repost reels
- Scrape reels from a specified set of instagram accounts
- Discord bot with three channels:
  - Employs 3 different channels
    - "status" to show the current status of the bot, this channel is shared between all accounts
    - "posted" to show the reels that have been reposted in the last 24 hours, this channel is also shared between all accounts
    - "bot_username" to show the reels that have been scraped, including the ones that are currently queued 
  - Notification system:
    - When the content queue is about to run out
    - When the instagram account is restricted and needs to be manually unlocked (as in, logging in to the instagram account and dismissing/solving the captcha), a convenient "Resume" button is then displayed on the bot status to easily resume the bot
- Advanced video duplication detection
  - Using perceptual hashing, the bot can detect if a video has already been reposted, and will not repost it again
- AWS S3 integration
  - All content will be automatically uploaded to an S3 bucket, and removed when it expires.
- Podman/Docker support
  - Using the provided Dockerfile, you can easily build and run the bot in a container, leveraging cargo-chef for faster builds
  - Run the container with ./run_container.sh

## Hardcoded values

There are some hardcoded values in the code that you will need to change to make the bot work properly. These are located at the top of the main.rs file, and are the following:
- MY_DISCORD_ID: Your discord user id
- GUILD_ID: The id of the discord server where the bot will be running
- POSTED_CHANNEL_ID: The id of the channel where the bot will post the reels that have been reposted
- STATUS_CHANNEL_ID: The id of the channel where the bot will post the current status

The rest of the hardcoded values are battle-tested and should work out of the box, you shouldn't change them unless you know what you're doing.

