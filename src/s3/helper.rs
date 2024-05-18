use std::collections::HashMap;

use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::Region;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::{IS_OFFLINE, S3_EXPIRATION_TIME};

//noinspection ALL
pub async fn upload_to_s3(credentials: &HashMap<String, String>, video_path: String, path_to_file: String, delete_from_local_storage: bool) -> Result<String, Box<dyn std::error::Error>> {
    let access_key = Some(credentials.get("s3_access_key").unwrap().as_str());
    let secret_key = Some(credentials.get("s3_secret_key").unwrap().as_str());

    let creds = Credentials::new(access_key, secret_key, None, None, None).unwrap();
    let bucket = s3::bucket::Bucket::new("repostrusty", Region::EuNorth1, creds).unwrap();

    let file_path = format!("temp/{}", video_path);
    //println!("Uploading file: {} to s3", file_path);
    let mut file = File::open(file_path.clone()).await.unwrap();
    let mut file_content = Vec::new();
    file.read_to_end(&mut file_content).await.unwrap();

    let mut final_path = path_to_file;
    if IS_OFFLINE {
        final_path = format!("dev/{}", final_path);
    }

    match bucket.put_object_with_content_type(final_path.clone(), &file_content, "video/mp4").await {
        Ok(_) => {}
        Err(e) => {
            tracing::warn!("Error uploading file to s3, retrying...\n{}", e);
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            match bucket.put_object_with_content_type(final_path.clone(), &file_content, "video/mp4").await {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("Error uploading file to s3: {}", e);
                    return Err(Box::new(e));
                }
            };
        }
    };
    let url = bucket.presign_get(final_path.clone(), S3_EXPIRATION_TIME, None).await.unwrap();

    if delete_from_local_storage {
        tokio::fs::remove_file(file_path).await.unwrap();
    }

    Ok(url)
}

pub async fn delete_from_s3(credentials: &HashMap<String, String>, path_to_file: String) -> Result<(), Box<dyn std::error::Error>> {
    let access_key = Some(credentials.get("s3_access_key").unwrap().as_str());
    let secret_key = Some(credentials.get("s3_secret_key").unwrap().as_str());

    let creds = Credentials::new(access_key, secret_key, None, None, None).unwrap();
    let bucket = Bucket::new("repostrusty", Region::EuNorth1, creds).unwrap();

    let mut final_path = path_to_file;
    if IS_OFFLINE {
        final_path = format!("dev/{}", final_path);
    }
    bucket.delete_object(final_path).await.unwrap();

    Ok(())
}
