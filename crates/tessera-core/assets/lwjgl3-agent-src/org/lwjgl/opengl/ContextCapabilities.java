package org.lwjgl.opengl;

import java.lang.reflect.Field;

/**
 * LWJGL 2's capability flags, answered from LWJGL 3's GLCapabilities.
 *
 * The game reads these fields directly, so the names have to match LWJGL 2
 * exactly. Only the fields 1.8.9 touches are declared: a missing flag is then a
 * compile-time error in the shim, not a silent "unsupported".
 *
 * LWJGL 3 stopped exposing a few of the old extension booleans because the
 * extension is core since GL 3.x. Those are set from {@link #CORE} below; the
 * rest are read off GLCapabilities by name.
 */
public final class ContextCapabilities {
    /** Extensions that are core in the compatibility profile 1.8.9 runs on. */
    private static final java.util.Set<String> CORE = java.util.Set.of(
            "GL_ARB_multitexture", "GL_ARB_texture_env_combine", "GL_ARB_texture_cube_map",
            "GL_ARB_depth_texture", "GL_ARB_shadow", "GL_ARB_shadow_ambient",
            "GL_ARB_texture_border_clamp", "GL_ARB_multisample",
            "GL_ARB_vertex_buffer_object", "GL_ARB_draw_buffers", "GL_ARB_shader_objects",
            "GL_ARB_fragment_shader", "GL_ARB_vertex_shader", "GL_ARB_framebuffer_object",
            "GL_ARB_pixel_buffer_object", "GL_ARB_texture_non_power_of_two",
            "GL_ARB_occlusion_query2", "GL_ARB_instanced_arrays", "GL_ARB_fragment_program",
            "GL_ARB_fragment_program_shadow", "GL_ARB_vertex_program", "GL_ARB_vertex_blend",
            "GL_EXT_texture_3d", "GL_EXT_texture_lod_bias", "GL_EXT_rescale_normal",
            "GL_EXT_multi_draw_arrays", "GL_EXT_blend_minmax", "GL_EXT_blend_subtract",
            "GL_EXT_blend_func_separate", "GL_EXT_blend_equation_separate",
            "GL_EXT_stencil_wrap", "GL_EXT_texture_sRGB", "GL_EXT_packed_depth_stencil",
            "GL_EXT_framebuffer_object", "GL_EXT_framebuffer_multisample",
            "GL_EXT_texture_array", "GL_EXT_texture_integer", "GL_EXT_gpu_shader4",
            "GL_EXT_geometry_shader4", "GL_EXT_separate_shader_objects",
            "GL_EXT_texture_buffer_object", "GL_EXT_shadow_funcs", "GL_EXT_stencil_two_side",
            "GL_EXT_stencil_clear_tag", "GL_EXT_bindable_uniform",
            "GL_EXT_gpu_program_parameters", "GL_NV_fog_distance");

    public final boolean GL_ARB_arrays_of_arrays;
    public final boolean GL_ARB_base_instance;
    public final boolean GL_ARB_blend_func_extended;
    public final boolean GL_ARB_clear_buffer_object;
    public final boolean GL_ARB_color_buffer_float;
    public final boolean GL_ARB_compatibility;
    public final boolean GL_ARB_compressed_texture_pixel_storage;
    public final boolean GL_ARB_compute_shader;
    public final boolean GL_ARB_copy_buffer;
    public final boolean GL_ARB_copy_image;
    public final boolean GL_ARB_depth_buffer_float;
    public final boolean GL_ARB_depth_clamp;
    public final boolean GL_ARB_depth_texture;
    public final boolean GL_ARB_draw_buffers;
    public final boolean GL_ARB_draw_buffers_blend;
    public final boolean GL_ARB_draw_elements_base_vertex;
    public final boolean GL_ARB_draw_indirect;
    public final boolean GL_ARB_draw_instanced;
    public final boolean GL_ARB_explicit_attrib_location;
    public final boolean GL_ARB_explicit_uniform_location;
    public final boolean GL_ARB_fragment_layer_viewport;
    public final boolean GL_ARB_fragment_program;
    public final boolean GL_ARB_fragment_program_shadow;
    public final boolean GL_ARB_fragment_shader;
    public final boolean GL_ARB_framebuffer_object;
    public final boolean GL_ARB_framebuffer_sRGB;
    public final boolean GL_ARB_geometry_shader4;
    public final boolean GL_ARB_gpu_shader5;
    public final boolean GL_ARB_half_float_pixel;
    public final boolean GL_ARB_half_float_vertex;
    public final boolean GL_ARB_instanced_arrays;
    public final boolean GL_ARB_map_buffer_alignment;
    public final boolean GL_ARB_map_buffer_range;
    public final boolean GL_ARB_multisample;
    public final boolean GL_ARB_multitexture;
    public final boolean GL_ARB_occlusion_query2;
    public final boolean GL_ARB_pixel_buffer_object;
    public final boolean GL_ARB_seamless_cube_map;
    public final boolean GL_ARB_shader_objects;
    public final boolean GL_ARB_shader_stencil_export;
    public final boolean GL_ARB_shader_texture_lod;
    public final boolean GL_ARB_shadow;
    public final boolean GL_ARB_shadow_ambient;
    public final boolean GL_ARB_stencil_texturing;
    public final boolean GL_ARB_sync;
    public final boolean GL_ARB_tessellation_shader;
    public final boolean GL_ARB_texture_border_clamp;
    public final boolean GL_ARB_texture_buffer_object;
    public final boolean GL_ARB_texture_cube_map;
    public final boolean GL_ARB_texture_cube_map_array;
    public final boolean GL_ARB_texture_env_combine;
    public final boolean GL_ARB_texture_non_power_of_two;
    public final boolean GL_ARB_uniform_buffer_object;
    public final boolean GL_ARB_vertex_blend;
    public final boolean GL_ARB_vertex_buffer_object;
    public final boolean GL_ARB_vertex_program;
    public final boolean GL_ARB_vertex_shader;
    public final boolean GL_EXT_bindable_uniform;
    public final boolean GL_EXT_blend_equation_separate;
    public final boolean GL_EXT_blend_func_separate;
    public final boolean GL_EXT_blend_minmax;
    public final boolean GL_EXT_blend_subtract;
    public final boolean GL_EXT_draw_instanced;
    public final boolean GL_EXT_framebuffer_multisample;
    public final boolean GL_EXT_framebuffer_object;
    public final boolean GL_EXT_framebuffer_sRGB;
    public final boolean GL_EXT_geometry_shader4;
    public final boolean GL_EXT_gpu_program_parameters;
    public final boolean GL_EXT_gpu_shader4;
    public final boolean GL_EXT_multi_draw_arrays;
    public final boolean GL_EXT_packed_depth_stencil;
    public final boolean GL_EXT_paletted_texture;
    public final boolean GL_EXT_rescale_normal;
    public final boolean GL_EXT_separate_shader_objects;
    public final boolean GL_EXT_shader_image_load_store;
    public final boolean GL_EXT_shadow_funcs;
    public final boolean GL_EXT_shared_texture_palette;
    public final boolean GL_EXT_stencil_clear_tag;
    public final boolean GL_EXT_stencil_two_side;
    public final boolean GL_EXT_stencil_wrap;
    public final boolean GL_EXT_texture_3d;
    public final boolean GL_EXT_texture_array;
    public final boolean GL_EXT_texture_buffer_object;
    public final boolean GL_EXT_texture_integer;
    public final boolean GL_EXT_texture_lod_bias;
    public final boolean GL_EXT_texture_sRGB;
    public final boolean GL_EXT_vertex_shader;
    public final boolean GL_EXT_vertex_weighting;
    public final boolean GL_NV_fog_distance;
    public final boolean OpenGL13;
    public final boolean OpenGL14;
    public final boolean OpenGL15;
    public final boolean OpenGL20;
    public final boolean OpenGL21;
    public final boolean OpenGL30;

    public ContextCapabilities(GLCapabilities capabilities) {
        this.GL_ARB_arrays_of_arrays = flag(capabilities, "GL_ARB_arrays_of_arrays");
        this.GL_ARB_base_instance = flag(capabilities, "GL_ARB_base_instance");
        this.GL_ARB_blend_func_extended = flag(capabilities, "GL_ARB_blend_func_extended");
        this.GL_ARB_clear_buffer_object = flag(capabilities, "GL_ARB_clear_buffer_object");
        this.GL_ARB_color_buffer_float = flag(capabilities, "GL_ARB_color_buffer_float");
        this.GL_ARB_compatibility = flag(capabilities, "GL_ARB_compatibility");
        this.GL_ARB_compressed_texture_pixel_storage = flag(capabilities, "GL_ARB_compressed_texture_pixel_storage");
        this.GL_ARB_compute_shader = flag(capabilities, "GL_ARB_compute_shader");
        this.GL_ARB_copy_buffer = flag(capabilities, "GL_ARB_copy_buffer");
        this.GL_ARB_copy_image = flag(capabilities, "GL_ARB_copy_image");
        this.GL_ARB_depth_buffer_float = flag(capabilities, "GL_ARB_depth_buffer_float");
        this.GL_ARB_depth_clamp = flag(capabilities, "GL_ARB_depth_clamp");
        this.GL_ARB_depth_texture = flag(capabilities, "GL_ARB_depth_texture") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_draw_buffers = flag(capabilities, "GL_ARB_draw_buffers") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_draw_buffers_blend = flag(capabilities, "GL_ARB_draw_buffers_blend");
        this.GL_ARB_draw_elements_base_vertex = flag(capabilities, "GL_ARB_draw_elements_base_vertex");
        this.GL_ARB_draw_indirect = flag(capabilities, "GL_ARB_draw_indirect");
        this.GL_ARB_draw_instanced = flag(capabilities, "GL_ARB_draw_instanced");
        this.GL_ARB_explicit_attrib_location = flag(capabilities, "GL_ARB_explicit_attrib_location");
        this.GL_ARB_explicit_uniform_location = flag(capabilities, "GL_ARB_explicit_uniform_location");
        this.GL_ARB_fragment_layer_viewport = flag(capabilities, "GL_ARB_fragment_layer_viewport");
        this.GL_ARB_fragment_program = flag(capabilities, "GL_ARB_fragment_program") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_fragment_program_shadow = flag(capabilities, "GL_ARB_fragment_program_shadow") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_fragment_shader = flag(capabilities, "GL_ARB_fragment_shader") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_framebuffer_object = flag(capabilities, "GL_ARB_framebuffer_object") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_framebuffer_sRGB = flag(capabilities, "GL_ARB_framebuffer_sRGB");
        this.GL_ARB_geometry_shader4 = flag(capabilities, "GL_ARB_geometry_shader4");
        this.GL_ARB_gpu_shader5 = flag(capabilities, "GL_ARB_gpu_shader5");
        this.GL_ARB_half_float_pixel = flag(capabilities, "GL_ARB_half_float_pixel");
        this.GL_ARB_half_float_vertex = flag(capabilities, "GL_ARB_half_float_vertex");
        this.GL_ARB_instanced_arrays = flag(capabilities, "GL_ARB_instanced_arrays") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_map_buffer_alignment = flag(capabilities, "GL_ARB_map_buffer_alignment");
        this.GL_ARB_map_buffer_range = flag(capabilities, "GL_ARB_map_buffer_range");
        this.GL_ARB_multisample = flag(capabilities, "GL_ARB_multisample") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_multitexture = flag(capabilities, "GL_ARB_multitexture") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_occlusion_query2 = flag(capabilities, "GL_ARB_occlusion_query2") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_pixel_buffer_object = flag(capabilities, "GL_ARB_pixel_buffer_object") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_seamless_cube_map = flag(capabilities, "GL_ARB_seamless_cube_map");
        this.GL_ARB_shader_objects = flag(capabilities, "GL_ARB_shader_objects") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_shader_stencil_export = flag(capabilities, "GL_ARB_shader_stencil_export");
        this.GL_ARB_shader_texture_lod = flag(capabilities, "GL_ARB_shader_texture_lod");
        this.GL_ARB_shadow = flag(capabilities, "GL_ARB_shadow") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_shadow_ambient = flag(capabilities, "GL_ARB_shadow_ambient") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_stencil_texturing = flag(capabilities, "GL_ARB_stencil_texturing");
        this.GL_ARB_sync = flag(capabilities, "GL_ARB_sync");
        this.GL_ARB_tessellation_shader = flag(capabilities, "GL_ARB_tessellation_shader");
        this.GL_ARB_texture_border_clamp = flag(capabilities, "GL_ARB_texture_border_clamp") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_texture_buffer_object = flag(capabilities, "GL_ARB_texture_buffer_object");
        this.GL_ARB_texture_cube_map = flag(capabilities, "GL_ARB_texture_cube_map") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_texture_cube_map_array = flag(capabilities, "GL_ARB_texture_cube_map_array");
        this.GL_ARB_texture_env_combine = flag(capabilities, "GL_ARB_texture_env_combine") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_texture_non_power_of_two = flag(capabilities, "GL_ARB_texture_non_power_of_two") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_uniform_buffer_object = flag(capabilities, "GL_ARB_uniform_buffer_object");
        this.GL_ARB_vertex_blend = flag(capabilities, "GL_ARB_vertex_blend") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_vertex_buffer_object = flag(capabilities, "GL_ARB_vertex_buffer_object") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_vertex_program = flag(capabilities, "GL_ARB_vertex_program") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_ARB_vertex_shader = flag(capabilities, "GL_ARB_vertex_shader") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_bindable_uniform = flag(capabilities, "GL_EXT_bindable_uniform") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_blend_equation_separate = flag(capabilities, "GL_EXT_blend_equation_separate") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_blend_func_separate = flag(capabilities, "GL_EXT_blend_func_separate") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_blend_minmax = flag(capabilities, "GL_EXT_blend_minmax") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_blend_subtract = flag(capabilities, "GL_EXT_blend_subtract") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_draw_instanced = flag(capabilities, "GL_EXT_draw_instanced");
        this.GL_EXT_framebuffer_multisample = flag(capabilities, "GL_EXT_framebuffer_multisample") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_framebuffer_object = flag(capabilities, "GL_EXT_framebuffer_object") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_framebuffer_sRGB = flag(capabilities, "GL_EXT_framebuffer_sRGB");
        this.GL_EXT_geometry_shader4 = flag(capabilities, "GL_EXT_geometry_shader4") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_gpu_program_parameters = flag(capabilities, "GL_EXT_gpu_program_parameters") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_gpu_shader4 = flag(capabilities, "GL_EXT_gpu_shader4") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_multi_draw_arrays = flag(capabilities, "GL_EXT_multi_draw_arrays") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_packed_depth_stencil = flag(capabilities, "GL_EXT_packed_depth_stencil") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_paletted_texture = flag(capabilities, "GL_EXT_paletted_texture");
        this.GL_EXT_rescale_normal = flag(capabilities, "GL_EXT_rescale_normal") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_separate_shader_objects = flag(capabilities, "GL_EXT_separate_shader_objects") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_shader_image_load_store = flag(capabilities, "GL_EXT_shader_image_load_store");
        this.GL_EXT_shadow_funcs = flag(capabilities, "GL_EXT_shadow_funcs") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_shared_texture_palette = flag(capabilities, "GL_EXT_shared_texture_palette");
        this.GL_EXT_stencil_clear_tag = flag(capabilities, "GL_EXT_stencil_clear_tag") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_stencil_two_side = flag(capabilities, "GL_EXT_stencil_two_side") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_stencil_wrap = flag(capabilities, "GL_EXT_stencil_wrap") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_texture_3d = flag(capabilities, "GL_EXT_texture_3d") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_texture_array = flag(capabilities, "GL_EXT_texture_array") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_texture_buffer_object = flag(capabilities, "GL_EXT_texture_buffer_object") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_texture_integer = flag(capabilities, "GL_EXT_texture_integer") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_texture_lod_bias = flag(capabilities, "GL_EXT_texture_lod_bias") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_texture_sRGB = flag(capabilities, "GL_EXT_texture_sRGB") || true; // core in the 3.x/4.x profile the game runs on
        this.GL_EXT_vertex_shader = flag(capabilities, "GL_EXT_vertex_shader");
        this.GL_EXT_vertex_weighting = flag(capabilities, "GL_EXT_vertex_weighting");
        this.GL_NV_fog_distance = flag(capabilities, "GL_NV_fog_distance") || true; // core in the 3.x/4.x profile the game runs on
        this.OpenGL13 = capabilities.OpenGL13;
        this.OpenGL14 = capabilities.OpenGL14;
        this.OpenGL15 = capabilities.OpenGL15;
        this.OpenGL20 = capabilities.OpenGL20;
        this.OpenGL21 = capabilities.OpenGL21;
        this.OpenGL30 = capabilities.OpenGL30;
    }

    /** A capability flag by name: GLCapabilities first, then the core table. */
    private static boolean flag(GLCapabilities capabilities, String name) {
        if (CORE.contains(name)) {
            return true;
        }
        try {
            Field field = GLCapabilities.class.getField(name);
            if (field.getType() == boolean.class) {
                return field.getBoolean(capabilities);
            }
            return field.get(capabilities) != null;
        } catch (NoSuchFieldException | IllegalAccessException e) {
            return false;
        }
    }
}
