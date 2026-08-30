use crate::video::{
    frames::FramePool,
    linux::{file::FileVideoStream, screengrab::ScreenVideoStream},
};

pub mod file;
pub mod screengrab;

pub enum ActiveVideoStream {
    Screen(ScreenVideoStream),
    File(FileVideoStream),
}

impl ActiveVideoStream {
    pub fn get_frame_pool(&mut self) -> &mut FramePool {
        match self {
            Self::Screen(stream) => &mut stream.frame_pool,
            Self::File(stream) => &mut stream.frame_pool,
        }
    }

    pub fn close(self) {
        match self {
            Self::Screen(stream) => {
                stream.close();
            }
            Self::File(_stream) => {}
        }
    }
}
