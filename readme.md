# repost_rusty

repost_rusty  is an automation tool developed in Rust, designed to simplify and streamline the process of reposting Instagram reels. The tool addresses the problem of the tedious and non-scalable task of manually reposting content on Instagram. It automates the process by scraping reels from Instagram and uploading them in a user-friendly and efficient manner. The tool is capable of managing multiple Instagram accounts simultaneously, each isolated from the others, with low memory usage.
## The problem
Finding content on instagram is really easy, but reposting it is not as trivial as it sounds. Normally, to accomplish this you would either use a dedicated app (there are tweaked apps, like Rocket for Instagram, that easily allow you to repost content), or you go through the pain and suffering of going to your pc, manually finding the video url and downloading it, then uploading it to instagram, setting up the caption, when it will post, etc. 

While this is a remarkably simple task, it is also a very tedious one, and it is not scalable at all.

## The solution
Just automate the tasks in your favorite language! Right?

Well to do that, in Rust, you would need to find a way to scrape reels from instagram, and upload them in an easy to use and high level way.

I searched far and wide for a crate that would allow me to do this, and I found a couple of them, but they were either outdated, or didn't work at all.

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

I picked the [Teloxide](https://github.com/teloxide/teloxide) crate and it worked great, until I realized that it really didn't. For some obscure way (or maybe not so obscure, I didn't really look into it), the chat with the bot would not get completely synchronized with all the devices, leaving dangling buttons that would make the bot crash.

This meant that the bot worked great if you had a device with telegram open all the time, but if you didn't, or if you used multiple devices, the issues above would arise. The user would be then forced to clear the chat and resend the /start message.

In my opinion this was not acceptable, as it kind of defeated the purpose of the bot, which was to make reposting reels as easy as possible.

I started looking for alternatives that would be cross-platform, easy to use and flexible, and I found the [Serenity](https://github.com/serenity-rs/serenity) crate, which is a discord bot framework for Rust.

Using the previous front-end code, I was able to pretty quickly make the switch to discord, and I am very happy with the results.

The discord bot is bullet-proof, it is always in sync with all the devices, it is easy to use, and it is cross-platform. It also has a clear advantage over the telegram bot, since you can use channels to separate the different types of messages (and the related notifications)

It wasn't completely smooth sailing however, behind the seemingly perfect discord bot, I was briefly plagued by a minor issue, which was that the buttons where extremely unreliable. They would work 3 times out of 10, for no apparent reason. 

This made the bot temporarily more annoying than the telegram one, as the user would have to spam the button until it worked, sometime take up to a minute.

I always suspected this was due to the how the bot fundamentally works: it spawns a new thread when the Ready event is called from Serenity, which would then handle the actual updates of the bot, such as updating the messages, checking if everything is alright, etc. however, the logic there always seemed sound to me, and I couldn't find any issues with it.

One day, after being extremely frustrated with the issue, literally the buttons were not working at all, I decided to take another deep look at the code, and started playing around with where the REFRESH_RATE sleep was located in the ready loop.

And well, that seemed to fix the issue. Huh.
While I can't say that the buttons now work 100% of the time, they work a decent 9 times out of 10.

Overall, I am very happy with the discord bot, and I think it is a great choice for this project.

## S3 integration

Initially the bot revolved around not storing the files at all, and just storing the urls of the reels in the database. This later on turned out to be a very bad idea, since the instagram urls come from cdns and they expire after a while, making both discord not display the content properly and the bot fail to repost the reels.

I then started to need to temporarily store the files for the video processing, and while I was there, I decided to use AWS S3 to store the files after being processed. I had never used S3 before, but I had heard good things about it, and I was not disappointed.

The integration was very easy, and the bot now stores the files in S3, and removes them when they expire.

## Video duplication detection

The bot uses perceptual hashing to detect if a video has already been reposted, and will not even show it to the user if it has. This is a very important feature, as it allows the bot to avoid reposting the same video multiple times, which would be very annoying for the followers of the account.

It works by hashing 4 frames, the first and last frame of the video, and two other frames in between. 

It then checks if the Hamming distance between the hashes is less than 2, and if it is then the videos are duplicated.

These two constants are totally arbitrary numbers, but they seem to work well enough, while keeping the performance impact low.

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
      - Here the user can choose to either accept, reject or edit the reel, offering maximum flexibility
      - Also integrates a near live countdown of the time left until the reels are reposted
  - Notification system:
    - When the content queue is about to run out
    - When the instagram account is restricted and needs to be manually unlocked (as in, logging in to the instagram account and dismissing/solving the captcha), a convenient "Resume" button is then displayed on the bot status to easily resume the bot
- Advanced video duplication detection
  - Using perceptual hashing, the bot can detect if a video has already been reposted, and will not even show it to the user if it has
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

