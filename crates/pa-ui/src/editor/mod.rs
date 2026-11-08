//! Photo editor (features/photo-editor): the document model and the shader that renders it on
//! the GPU (wgpu) or the CPU.

#![allow(dead_code)]

mod doc;
#[cfg(feature = "gpu")]
mod gpu;
mod render;
