use context::{self, GlVersion};
use gl;
use libc;
use std::{fmt, mem, ptr, slice};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::ops::{Deref, DerefMut};
use GlObject;

/// A buffer in the graphics card's memory.
pub struct Buffer {
    display: Arc<super::DisplayImpl>,
    id: gl::types::GLuint,
    elements_size: uint,
    elements_count: uint,
}

/// Type of a buffer.
pub trait BufferType {
    /// Should return `&mut ctxt.state.something`.
    fn get_storage_point(Option<Self>, &mut context::GLState)
        -> &mut gl::types::GLuint;
    /// Should return `gl::SOMETHING_BUFFER`.
    fn get_bind_point(Option<Self>) -> gl::types::GLenum;
}

/// Used for vertex buffers.
pub struct ArrayBuffer;

impl BufferType for ArrayBuffer {
    fn get_storage_point(_: Option<ArrayBuffer>, state: &mut context::GLState)
        -> &mut gl::types::GLuint
    {
        &mut state.array_buffer_binding
    }

    fn get_bind_point(_: Option<ArrayBuffer>) -> gl::types::GLenum {
        gl::ARRAY_BUFFER
    }
}

/// Used for pixel buffers.
pub struct PixelPackBuffer;

impl BufferType for PixelPackBuffer {
    fn get_storage_point(_: Option<PixelPackBuffer>, state: &mut context::GLState)
        -> &mut gl::types::GLuint
    {
        &mut state.pixel_pack_buffer_binding
    }

    fn get_bind_point(_: Option<PixelPackBuffer>) -> gl::types::GLenum {
        gl::PIXEL_PACK_BUFFER
    }
}

/// Used for pixel buffers.
pub struct PixelUnpackBuffer;

impl BufferType for PixelUnpackBuffer {
    fn get_storage_point(_: Option<PixelUnpackBuffer>, state: &mut context::GLState)
        -> &mut gl::types::GLuint
    {
        &mut state.pixel_unpack_buffer_binding
    }

    fn get_bind_point(_: Option<PixelUnpackBuffer>) -> gl::types::GLenum {
        gl::PIXEL_UNPACK_BUFFER
    }
}

impl Buffer {
    pub fn new<T, D>(display: &super::Display, data: Vec<D>, usage: gl::types::GLenum)
        -> Buffer where T: BufferType, D: Send + Copy
    {
        use std::mem;

        let elements_size = if data.len() <= 1 {
            mem::size_of::<D>()
        } else {
            let d0: *const D = &data[0];
            let d1: *const D = &data[1];
            (d1 as uint) - (d0 as uint)
        };

        let elements_count = data.len();
        let buffer_size = elements_count * elements_size as uint;

        let (tx, rx) = channel();

        display.context.context.exec(move |: ctxt| {
            let data = data;

            unsafe {
                let mut id: gl::types::GLuint = mem::uninitialized();
                ctxt.gl.GenBuffers(1, &mut id);
                tx.send(id).unwrap();

                let storage = BufferType::get_storage_point(None::<T>, ctxt.state);
                let bind = BufferType::get_bind_point(None::<T>);

                ctxt.gl.BindBuffer(bind, id);
                *storage = id;

                if ctxt.version >= &GlVersion(4, 4) || ctxt.extensions.gl_arb_buffer_storage {
                    ctxt.gl.BufferStorage(bind, buffer_size as gl::types::GLsizeiptr,
                                          data.as_ptr() as *const libc::c_void,
                                          gl::DYNAMIC_STORAGE_BIT | gl::MAP_READ_BIT |
                                          gl::MAP_WRITE_BIT);       // TODO: more specific flags
                } else {
                    ctxt.gl.BufferData(bind, buffer_size as gl::types::GLsizeiptr,
                                       data.as_ptr() as *const libc::c_void, usage);
                }

                let mut obtained_size: gl::types::GLint = mem::uninitialized();
                ctxt.gl.GetBufferParameteriv(bind, gl::BUFFER_SIZE, &mut obtained_size);
                if buffer_size != obtained_size as uint {
                    ctxt.gl.DeleteBuffers(1, [id].as_ptr());
                    panic!("Not enough available memory for buffer");
                }
            }
        });

        Buffer {
            display: display.context.clone(),
            id: rx.recv().unwrap(),
            elements_size: elements_size,
            elements_count: elements_count,
        }
    }

    pub fn new_empty<T>(display: &super::Display, elements_size: uint, elements_count: uint,
                        usage: gl::types::GLenum) -> Buffer where T: BufferType
    {
        let buffer_size = elements_count * elements_size as uint;

        let (tx, rx) = channel();
        display.context.context.exec(move |: ctxt| {
            unsafe {
                let mut id: gl::types::GLuint = mem::uninitialized();
                ctxt.gl.GenBuffers(1, &mut id);

                let storage = BufferType::get_storage_point(None::<T>, ctxt.state);
                let bind = BufferType::get_bind_point(None::<T>);

                ctxt.gl.BindBuffer(bind, id);
                *storage = id;

                if ctxt.version >= &GlVersion(4, 4) || ctxt.extensions.gl_arb_buffer_storage {
                    ctxt.gl.BufferStorage(bind, buffer_size as gl::types::GLsizeiptr,
                                          ptr::null(),
                                          gl::DYNAMIC_STORAGE_BIT | gl::MAP_READ_BIT |
                                          gl::MAP_WRITE_BIT);       // TODO: more specific flags
                } else {
                    ctxt.gl.BufferData(bind, buffer_size as gl::types::GLsizeiptr,
                                       ptr::null(), usage);
                }

                let mut obtained_size: gl::types::GLint = mem::uninitialized();
                ctxt.gl.GetBufferParameteriv(bind, gl::BUFFER_SIZE, &mut obtained_size);
                if buffer_size != obtained_size as uint {
                    ctxt.gl.DeleteBuffers(1, [id].as_ptr());
                    panic!("Not enough available memory for buffer");
                }

                tx.send(id).unwrap();
            }
        });

        Buffer {
            display: display.context.clone(),
            id: rx.recv().unwrap(),
            elements_size: elements_size,
            elements_count: elements_count,
        }
    }

    pub fn get_display(&self) -> &Arc<super::DisplayImpl> {
        &self.display
    }

    pub fn get_elements_size(&self) -> uint {
        self.elements_size
    }

    pub fn get_elements_count(&self) -> uint {
        self.elements_count
    }

    pub fn get_total_size(&self) -> uint {
        self.elements_count * self.elements_size
    }

    /// Offset and size are in number of elements
    pub fn map<'a, T, D>(&'a mut self, offset: uint, size: uint)
                         -> Mapping<'a, T, D> where T: BufferType, D: Send
    {
        let (tx, rx) = channel();
        let id = self.id.clone();

        if offset > self.elements_count || (offset + size) > self.elements_count {
            panic!("Trying to map out of range of buffer");
        }

        let offset_bytes = offset * self.elements_size;
        let size_bytes = size * self.elements_size;

        self.display.context.exec(move |: ctxt| {
            let ptr = unsafe {
                if ctxt.version >= &GlVersion(4, 5) {
                    ctxt.gl.MapNamedBufferRange(id, offset_bytes as gl::types::GLintptr,
                                                size_bytes as gl::types::GLsizei,
                                                gl::MAP_READ_BIT | gl::MAP_WRITE_BIT)

                } else {
                    let storage = BufferType::get_storage_point(None::<T>, ctxt.state);
                    let bind = BufferType::get_bind_point(None::<T>);

                    ctxt.gl.BindBuffer(bind, id);
                    *storage = id;
                    ctxt.gl.MapBufferRange(bind, offset_bytes as gl::types::GLintptr,
                                           size_bytes as gl::types::GLsizeiptr,
                                           gl::MAP_READ_BIT | gl::MAP_WRITE_BIT)
                }
            };

            tx.send(ptr::Unique(ptr as *mut D)).unwrap();
        });

        Mapping {
            buffer: self,
            data: rx.recv().unwrap().0,
            len: size,
        }
    }

    #[cfg(feature = "gl_extensions")]
    pub fn read<T, D>(&self) -> Vec<D> where T: BufferType, D: Send {
        self.read_slice::<T, D>(0, self.elements_count)
    }

    #[cfg(feature = "gl_extensions")]
    pub fn read_slice<T, D>(&self, offset: uint, size: uint)
                            -> Vec<D> where T: BufferType, D: Send
    {
        assert!(offset + size <= self.elements_count);

        let id = self.id.clone();
        let elements_size = self.elements_size.clone();
        let (tx, rx) = channel();

        self.display.context.exec(move |: ctxt| {
            if ctxt.opengl_es {
                panic!("OpenGL ES doesn't support glGetBufferSubData");
            }

            unsafe {
                let mut data = Vec::with_capacity(size);
                data.set_len(size);

                if !ctxt.opengl_es && ctxt.version >= &GlVersion(4, 5) {
                    ctxt.gl.GetNamedBufferSubData(id, (offset * elements_size) as gl::types::GLintptr,
                        (size * elements_size) as gl::types::GLsizei,
                        data.as_mut_ptr() as *mut libc::c_void);

                } else {
                    let storage = BufferType::get_storage_point(None::<T>, ctxt.state);
                    let bind = BufferType::get_bind_point(None::<T>);

                    ctxt.gl.BindBuffer(bind, id);
                    *storage = id;
                    ctxt.gl.GetBufferSubData(bind, (offset * elements_size) as gl::types::GLintptr,
                        (size * elements_size) as gl::types::GLsizeiptr,
                        data.as_mut_ptr() as *mut libc::c_void);
                }

                tx.send(data).ok();
            }
        });

        rx.recv().unwrap()
    }
}

impl fmt::Show for Buffer {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        (format!("Buffer #{}", self.id)).fmt(formatter)
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        let id = self.id.clone();
        self.display.context.exec(move |: ctxt| {
            if ctxt.state.array_buffer_binding == id {
                ctxt.state.array_buffer_binding = 0;
            }

            if ctxt.state.pixel_pack_buffer_binding == id {
                ctxt.state.pixel_pack_buffer_binding = 0;
            }

            if ctxt.state.pixel_unpack_buffer_binding == id {
                ctxt.state.pixel_unpack_buffer_binding = 0;
            }

            unsafe { ctxt.gl.DeleteBuffers(1, [ id ].as_ptr()); }
        });
    }
}

impl GlObject for Buffer {
    fn get_id(&self) -> gl::types::GLuint {
        self.id
    }
}

/// A mapping of a buffer.
pub struct Mapping<'b, T, D> {
    buffer: &'b mut Buffer,
    data: *mut D,
    len: uint,
}

#[unsafe_destructor]
impl<'a, T, D> Drop for Mapping<'a, T, D> where T: BufferType {
    fn drop(&mut self) {
        let id = self.buffer.id.clone();
        self.buffer.display.context.exec(move |: ctxt| {
            unsafe {
                if ctxt.version >= &GlVersion(4, 5) {
                    ctxt.gl.UnmapNamedBuffer(id);

                } else {
                    let storage = BufferType::get_storage_point(None::<T>, ctxt.state);
                    let bind = BufferType::get_bind_point(None::<T>);

                    if *storage != id {
                        ctxt.gl.BindBuffer(bind, id);
                        *storage = id;
                    }

                    ctxt.gl.UnmapBuffer(bind);
                }
            }
        });
    }
}

impl<'a, T, D> Deref for Mapping<'a, T, D> {
    type Target = [D];
    fn deref<'b>(&'b self) -> &'b [D] {
        unsafe {
            slice::from_raw_mut_buf(&self.data, self.len)
        }
    }
}

impl<'a, T, D> DerefMut for Mapping<'a, T, D> {
    fn deref_mut<'b>(&'b mut self) -> &'b mut [D] {
        unsafe {
            slice::from_raw_mut_buf(&self.data, self.len)
        }
    }
}
