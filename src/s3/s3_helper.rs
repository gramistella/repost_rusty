use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::Region;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::IS_OFFLINE;

//noinspection ALL
pub async fn upload_to_s3(video_path: String, path_to_file: String, delete_from_local_storage: bool) -> Result<String, Box<dyn std::error::Error>> {
    let access_key = Some("AKIAYS2NP32W7GLE6U57");
    let secret_access_key = Some("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");
    let creds = Credentials::new(access_key, secret_access_key, None, None, None).unwrap();
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

    bucket.put_object_with_content_type(final_path.clone(), &file_content, "video/mp4").await.unwrap();
    let url = bucket.presign_get(final_path.clone(), 60 * 60 * 24 * 7, None).await.unwrap();

    if delete_from_local_storage {
        tokio::fs::remove_file(file_path).await.unwrap();
    }

    Ok(url)
}

pub async fn delete_from_s3(path_to_file: String) -> Result<(), Box<dyn std::error::Error>> {
    let access_key = Some("AKIAYS2NP32W7GLE6U57");
    let secret_access_key = Some("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");
    let creds = Credentials::new(access_key, secret_access_key, None, None, None).unwrap();
    let bucket = Bucket::new("repostrusty", Region::EuNorth1, creds).unwrap();

    let mut final_path = path_to_file;
    if IS_OFFLINE {
        final_path = format!("dev/{}", final_path);
    }
    bucket.delete_object(final_path).await.unwrap();

    Ok(())
}
