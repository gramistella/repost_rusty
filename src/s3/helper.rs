use s3::bucket::Bucket;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::{IS_OFFLINE, S3_EXPIRATION_TIME};

//noinspection ALL
pub async fn upload_to_s3(bucket: &Bucket, video_path: String, path_to_file: String, delete_from_local_storage: bool) -> Result<String, Box<dyn std::error::Error>> {
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

pub async fn delete_from_s3(bucket: &Bucket, path_to_file: String) -> Result<(), Box<dyn std::error::Error>> {
    let mut final_path = path_to_file;
    if IS_OFFLINE {
        final_path = format!("dev/{}", final_path);
    }
    bucket.delete_object(final_path).await.unwrap();

    Ok(())
}

pub async fn update_presigned_url(bucket: &Bucket, path_to_file: String) -> Result<String, Box<dyn std::error::Error>> {
    let mut final_path = path_to_file;
    if IS_OFFLINE {
        final_path = format!("dev/{}", final_path);
    }

    let url = bucket.presign_get(final_path.clone(), S3_EXPIRATION_TIME, None).await.unwrap();

    Ok(url)
}
