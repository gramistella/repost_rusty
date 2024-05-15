use std::process::Command;
use std::process::Stdio;

use image_hasher::HasherConfig;

use crate::database::database::{DatabaseTransaction, HashedVideo};

fn divide_number(n: i32) -> [i32; 4] {
    let part1 = 0;
    let part2 = n / 3;
    let part3 = 2 * (n / 3);
    let part4 = n - 1;

    [part1, part2, part3, part4]
}

/// Returns whether the video already exists in the database

pub async fn process_video(tx: &mut DatabaseTransaction, video_path: &str, username: String, shortcode: String) -> Result<bool, Box<dyn std::error::Error>> {
    //println!("Processing video: {}, shortcode {}, username {}", video_path, shortcode, username);
    let path = format!("temp/{video_path}");

    let duration_seconds = get_video_duration(&path).unwrap();
    let total_frames = get_total_frames(&path).unwrap();

    let [frame1, frame2, frame3, frame4] = divide_number(total_frames);

    let frame_1_path = format!("temp/{}1.png", video_path);
    let frame_2_path = format!("temp/{}2.png", video_path);
    let frame_3_path = format!("temp/{}3.png", video_path);
    let frame_4_path = format!("temp/{}4.png", video_path);

    // Extract frames using ffmpeg command line
    extract_frame(&path, frame1, &frame_1_path)?;
    extract_frame(&path, frame2, &frame_2_path)?;
    extract_frame(&path, frame3, &frame_3_path)?;
    extract_frame(&path, frame4, &frame_4_path)?;

    let image1 = image::open(&frame_1_path).unwrap();
    let image2 = image::open(&frame_2_path).unwrap();
    let image3 = image::open(&frame_3_path).unwrap();
    let image4 = image::open(&frame_4_path).unwrap();

    let hasher = HasherConfig::new().to_hasher();

    let hash1 = hasher.hash_image(&image1);
    let hash2 = hasher.hash_image(&image2);
    let hash3 = hasher.hash_image(&image3);
    let hash4 = hasher.hash_image(&image4);

    let hashed_videos = tx.load_hashed_videos().unwrap();

    let mut video_exists = false;
    for hashed_video in hashed_videos {
        if hashed_video.duration != duration_seconds {
            continue;
        }

        let dist1 = hashed_video.hash_frame_1.dist(&hash1);
        let dist2 = hashed_video.hash_frame_2.dist(&hash2);
        let dist3 = hashed_video.hash_frame_3.dist(&hash3);
        let dist4 = hashed_video.hash_frame_4.dist(&hash4);

        let avg_dist = (dist1 + dist2 + dist3 + dist4) / 4;

        if avg_dist <= 3 {
            video_exists = true;
        }
    }

    if !video_exists {
        let video_hash = HashedVideo {
            username,
            duration: duration_seconds,
            original_shortcode: shortcode,
            hash_frame_1: hash1.clone(),
            hash_frame_2: hash2.clone(),
            hash_frame_3: hash3.clone(),
            hash_frame_4: hash4.clone(),
        };

        tx.save_hashed_video(video_hash).unwrap();
    }

    // Delete the extracted frames
    tokio::fs::remove_file(&frame_1_path).await.unwrap();
    tokio::fs::remove_file(&frame_2_path).await.unwrap();
    tokio::fs::remove_file(&frame_3_path).await.unwrap();
    tokio::fs::remove_file(&frame_4_path).await.unwrap();

    Ok(video_exists)
}

fn get_total_frames(video_path: &str) -> Result<i32, Box<dyn std::error::Error>> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=nb_frames")
        .arg("-of")
        .arg("default=nokey=1:noprint_wrappers=1")
        .arg(video_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    let total_frames = String::from_utf8(output.stdout).unwrap().trim().parse::<i32>().unwrap();
    Ok(total_frames)
}

fn get_video_duration(video_path: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(video_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    let duration = String::from_utf8(output.stdout).unwrap().trim().parse::<f64>().unwrap();
    Ok((duration * 1000.0).round() / 1000.0)
}

fn extract_frame(video_path: &str, frame_number: i32, output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(video_path)
        .arg("-vf")
        .arg(format!("select=eq(n\\,{})", frame_number))
        .arg("-vframes")
        .arg("1")
        .arg(output_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .unwrap();

    if !status.success() {
        return Err("Failed to extract frame".into());
    }

    Ok(())
}
