use thiserror::Error;

pub type VideoProcessingResult<T> = Result<T, VideoProcessingError>;

#[derive(Error, Debug)]
pub enum VideoProcessingError {
    #[error("Duration not returned by ffmpeg! Full output: {0}")]
    DurationError(String),
    #[error("Failed to extract frame {0} from video!")]
    FrameExtractionError(i32),
}
