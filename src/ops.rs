use std::sync::Arc;
use std::sync::mpsc::channel;

use Display;

use fbo::{self, FramebufferAttachments};

use uniforms::{Uniforms, UniformValue, SamplerBehavior};
use {DisplayImpl, Program, DrawParameters, Rect, Surface, GlObject, ToGlEnum};
use index_buffer::IndicesSource;
use vertex_buffer::VerticesSource;

use {program, vertex_array_object};
use {gl, context};

/// Draws everything.
pub fn draw<'a, I, U>(display: &Display,
    framebuffer: Option<&FramebufferAttachments>, vertex_buffer: VerticesSource,
    indices: &IndicesSource<I>, program: &Program, uniforms: U, draw_parameters: &DrawParameters,
    dimensions: (u32, u32)) where U: Uniforms, I: ::index_buffer::Index
{
    let fbo_id = fbo::get_framebuffer(&display.context, framebuffer);

    let vao_id = vertex_array_object::get_vertex_array_object(&display.context, vertex_buffer.clone(),
                                                              indices, program);

    let pointer = ::std::ptr::Unique(match indices {
        &IndicesSource::IndexBuffer { .. } => ::std::ptr::null_mut(),
        &IndicesSource::Buffer { ref pointer, .. } => pointer.as_ptr() as *mut ::libc::c_void,
    });

    let primitives = indices.get_primitives_type().to_glenum();
    let data_type = indices.get_indices_type().to_glenum();
    assert!(indices.get_offset() == 0); // not yet implemented
    let indices_count = indices.get_length();

    // building the list of uniforms binders
    let uniforms: Vec<Box<Fn(&mut context::CommandContext) + Send>> = {
        let uniforms_locations = program::get_uniforms_locations(program);
        let mut active_texture = 0;

        let mut uniforms_storage = Vec::new();
        uniforms.visit_values(|&mut: name, value| {
            if let Some(uniform) = uniforms_locations.get(name) {
                assert!(uniform.size.is_none());     // TODO: arrays not supported

                if !value.is_usable_with(&uniform.ty) {
                    panic!("Uniform value of type `{}` can't be bind to type `{}`",
                           value, uniform.ty);
                }

                let binder = uniform_to_binder(display, *value, uniform.location, &mut active_texture);
                uniforms_storage.push(binder);
            }
        });

        uniforms_storage
    };
    // TODO: panick if uniforms of the program are not found in the parameter

    let draw_parameters = draw_parameters.clone();

    let VerticesSource::VertexBuffer(vertex_buffer) = vertex_buffer;
    let vb_id = vertex_buffer.get_id();
    let program_id = program.get_id();

    // in some situations, we have to wait for the draw command to finish before returning
    let (tx, rx) = {
        let needs_sync = if let &IndicesSource::Buffer{..} = indices {
            true
        } else {
            false
        };

        if needs_sync {
            let (tx, rx) = channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        }
    };

    display.context.context.exec(move |: mut ctxt| {
        unsafe {
            fbo::bind_framebuffer(&mut ctxt, fbo_id, true, false);

            // binding program
            if ctxt.state.program != program_id {
                ctxt.gl.UseProgram(program_id);
                ctxt.state.program = program_id;
            }

            // binding program uniforms
            for binder in uniforms.into_iter() {
                binder.call((&mut ctxt,));
            }

            // binding VAO
            if ctxt.state.vertex_array != vao_id {
                ctxt.gl.BindVertexArray(vao_id);
                ctxt.state.vertex_array = vao_id;
            }

            // binding vertex buffer
            if ctxt.state.array_buffer_binding != vb_id {
                ctxt.gl.BindBuffer(gl::ARRAY_BUFFER, vb_id);
                ctxt.state.array_buffer_binding = vb_id;
            }

            // sync-ing parameters
            draw_parameters.sync(&mut ctxt, dimensions);

            // drawing
            ctxt.gl.DrawElements(primitives, indices_count as i32, data_type, pointer.0);
        }

        // sync-ing if necessary
        if let Some(tx) = tx {
            tx.send(()).ok();
        }
    });

    // sync-ing if necessary
    if let Some(rx) = rx {
        rx.recv().unwrap();
    }
}

pub fn clear_color(display: &Arc<DisplayImpl>, framebuffer: Option<&FramebufferAttachments>,
    red: f32, green: f32, blue: f32, alpha: f32)
{
    let fbo_id = fbo::get_framebuffer(display, framebuffer);

    let (red, green, blue, alpha) = (
        red as gl::types::GLclampf,
        green as gl::types::GLclampf,
        blue as gl::types::GLclampf,
        alpha as gl::types::GLclampf
    );

    display.context.exec(move |: mut ctxt| {
        fbo::bind_framebuffer(&mut ctxt, fbo_id, true, false);

        unsafe {
            if ctxt.state.clear_color != (red, green, blue, alpha) {
                ctxt.gl.ClearColor(red, green, blue, alpha);
                ctxt.state.clear_color = (red, green, blue, alpha);
            }

            ctxt.gl.Clear(gl::COLOR_BUFFER_BIT);
        }
    });
}

pub fn clear_depth(display: &Arc<DisplayImpl>, framebuffer: Option<&FramebufferAttachments>,
    value: f32)
{
    let value = value as gl::types::GLclampf;
    let fbo_id = fbo::get_framebuffer(display, framebuffer);

    display.context.exec(move |: mut ctxt| {
        fbo::bind_framebuffer(&mut ctxt, fbo_id, true, false);

        unsafe {
            if ctxt.state.clear_depth != value {
                ctxt.gl.ClearDepth(value as f64);        // TODO: find out why this needs "as"
                ctxt.state.clear_depth = value;
            }

            ctxt.gl.Clear(gl::DEPTH_BUFFER_BIT);
        }
    });
}

pub fn clear_stencil(display: &Arc<DisplayImpl>, framebuffer: Option<&FramebufferAttachments>,
    value: int)
{
    let value = value as gl::types::GLint;
    let fbo_id = fbo::get_framebuffer(display, framebuffer);

    display.context.exec(move |: mut ctxt| {
        fbo::bind_framebuffer(&mut ctxt, fbo_id, true, false);

        unsafe {
            if ctxt.state.clear_stencil != value {
                ctxt.gl.ClearStencil(value);
                ctxt.state.clear_stencil = value;
            }

            ctxt.gl.Clear(gl::STENCIL_BUFFER_BIT);
        }
    });
}

pub fn blit<S1: Surface, S2: Surface>(source: &S1, target: &S2, mask: gl::types::GLbitfield,
    src_rect: &Rect, target_rect: &Rect, filter: gl::types::GLenum)
{
    let ::BlitHelper(display, source) = source.get_blit_helper();
    let ::BlitHelper(_, target) = target.get_blit_helper();

    let src_rect = src_rect.clone();
    let target_rect = target_rect.clone();

    let source = fbo::get_framebuffer(display, source);
    let target = fbo::get_framebuffer(display, target);

    display.context.exec(move |: ctxt| {
        unsafe {
            // trying to do a named blit if possible
            if ctxt.version >= &context::GlVersion(4, 5) {
                ctxt.gl.BlitNamedFramebuffer(source.unwrap_or(0), target.unwrap_or(0),
                    src_rect.left as gl::types::GLint,
                    src_rect.bottom as gl::types::GLint,
                    (src_rect.left + src_rect.width) as gl::types::GLint,
                    (src_rect.bottom + src_rect.height) as gl::types::GLint,
                    target_rect.left as gl::types::GLint, target_rect.bottom as gl::types::GLint,
                    (target_rect.left + target_rect.width) as gl::types::GLint,
                    (target_rect.bottom + target_rect.height) as gl::types::GLint, mask, filter);

                return;
            }

            // binding source framebuffer
            if ctxt.state.read_framebuffer != source.unwrap_or(0) {
                if ctxt.version >= &context::GlVersion(3, 0) {
                    ctxt.gl.BindFramebuffer(gl::READ_FRAMEBUFFER, source.unwrap_or(0));
                    ctxt.state.read_framebuffer = source.unwrap_or(0);

                } else {
                    ctxt.gl.BindFramebufferEXT(gl::READ_FRAMEBUFFER_EXT, source.unwrap_or(0));
                    ctxt.state.read_framebuffer = source.unwrap_or(0);
                }
            }

            // binding target framebuffer
            if ctxt.state.draw_framebuffer != target.unwrap_or(0) {
                if ctxt.version >= &context::GlVersion(3, 0) {
                    ctxt.gl.BindFramebuffer(gl::DRAW_FRAMEBUFFER, target.unwrap_or(0));
                    ctxt.state.draw_framebuffer = target.unwrap_or(0);

                } else {
                    ctxt.gl.BindFramebufferEXT(gl::DRAW_FRAMEBUFFER_EXT, target.unwrap_or(0));
                    ctxt.state.draw_framebuffer = target.unwrap_or(0);
                }
            }

            // doing the blit
            if ctxt.version >= &context::GlVersion(3, 0) {
                ctxt.gl.BlitFramebuffer(src_rect.left as gl::types::GLint,
                    src_rect.bottom as gl::types::GLint,
                    (src_rect.left + src_rect.width) as gl::types::GLint,
                    (src_rect.bottom + src_rect.height) as gl::types::GLint,
                    target_rect.left as gl::types::GLint, target_rect.bottom as gl::types::GLint,
                    (target_rect.left + target_rect.width) as gl::types::GLint,
                    (target_rect.bottom + target_rect.height) as gl::types::GLint, mask, filter);

            } else {
                ctxt.gl.BlitFramebufferEXT(src_rect.left as gl::types::GLint,
                    src_rect.bottom as gl::types::GLint,
                    (src_rect.left + src_rect.width) as gl::types::GLint,
                    (src_rect.bottom + src_rect.height) as gl::types::GLint,
                    target_rect.left as gl::types::GLint, target_rect.bottom as gl::types::GLint,
                    (target_rect.left + target_rect.width) as gl::types::GLint,
                    (target_rect.bottom + target_rect.height) as gl::types::GLint, mask, filter);
            }
        }
    });
}

// TODO: we use a `Fn` instead of `FnOnce` because of that "std::thunk" issue
fn uniform_to_binder(display: &Display, value: UniformValue, location: gl::types::GLint,
                     active_texture: &mut gl::types::GLenum)
                     -> Box<Fn(&mut context::CommandContext) + Send>
{
    match value {
        UniformValue::SignedInt(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.Uniform1i(location, val)
                }
            }
        },
        UniformValue::UnsignedInt(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.Uniform1ui(location, val)
                }
            }
        },
        UniformValue::Float(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.Uniform1f(location, val)
                }
            }
        },
        UniformValue::Mat2(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.UniformMatrix2fv(location, 1, 0, val.as_ptr() as *const f32)
                }
            }
        },
        UniformValue::Mat3(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.UniformMatrix3fv(location, 1, 0, val.as_ptr() as *const f32)
                }
            }
        },
        UniformValue::Mat4(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.UniformMatrix4fv(location, 1, 0, val.as_ptr() as *const f32)
                }
            }
        },
        UniformValue::Vec2(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.Uniform2fv(location, 1, val.as_ptr() as *const f32)
                }
            }
        },
        UniformValue::Vec3(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.Uniform3fv(location, 1, val.as_ptr() as *const f32)
                }
            }
        },
        UniformValue::Vec4(val) => {
            box move |&: ctxt| {
                unsafe {
                    ctxt.gl.Uniform4fv(location, 1, val.as_ptr() as *const f32)
                }
            }
        },
        UniformValue::Texture1d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D)
        },
        UniformValue::CompressedTexture1d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D)
        },
        UniformValue::IntegralTexture1d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D)
        },
        UniformValue::UnsignedTexture1d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D)
        },
        UniformValue::DepthTexture1d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D)
        },
        UniformValue::Texture2d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D)
        },
        UniformValue::CompressedTexture2d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D)
        },
        UniformValue::IntegralTexture2d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D)
        },
        UniformValue::UnsignedTexture2d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D)
        },
        UniformValue::DepthTexture2d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D)
        },
        UniformValue::Texture3d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_3D)
        },
        UniformValue::CompressedTexture3d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_3D)
        },
        UniformValue::IntegralTexture3d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_3D)
        },
        UniformValue::UnsignedTexture3d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_3D)
        },
        UniformValue::DepthTexture3d(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_3D)
        },
        UniformValue::Texture1dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D_ARRAY)
        },
        UniformValue::CompressedTexture1dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D_ARRAY)
        },
        UniformValue::IntegralTexture1dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D_ARRAY)
        },
        UniformValue::UnsignedTexture1dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D_ARRAY)
        },
        UniformValue::DepthTexture1dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_1D_ARRAY)
        },
        UniformValue::Texture2dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D_ARRAY)
        },
        UniformValue::CompressedTexture2dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D_ARRAY)
        },
        UniformValue::IntegralTexture2dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D_ARRAY)
        },
        UniformValue::UnsignedTexture2dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D_ARRAY)
        },
        UniformValue::DepthTexture2dArray(texture, sampler) => {
            let texture = texture.get_id();
            build_texture_binder(display, texture, sampler, location, active_texture, gl::TEXTURE_2D_ARRAY)
        },
    }
}

fn build_texture_binder(display: &Display, texture: gl::types::GLuint,
                        sampler: Option<SamplerBehavior>, location: gl::types::GLint,
                        active_texture: &mut gl::types::GLenum,
                        bind_point: gl::types::GLenum)
                        -> Box<Fn(&mut context::CommandContext) + Send>
{
    assert!(*active_texture < display.context.context.capabilities()
                                     .max_combined_texture_image_units as gl::types::GLenum);

    let sampler = sampler.map(|b| ::uniforms::get_sampler(display, &b));

    let current_texture = *active_texture;
    *active_texture += 1;

    box move |&: ctxt| {
        unsafe {
            ctxt.gl.ActiveTexture(current_texture + gl::TEXTURE0);
            ctxt.gl.BindTexture(bind_point, texture);
            ctxt.gl.Uniform1i(location, current_texture as gl::types::GLint);

            if let Some(sampler) = sampler {
                ctxt.gl.BindSampler(current_texture, sampler);
            } else {
                ctxt.gl.BindSampler(current_texture, 0);
            }
        }
    }
}
