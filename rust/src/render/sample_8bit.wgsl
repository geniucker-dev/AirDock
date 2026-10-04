fn sample_plane(plane:texture_2d<f32>,uv:vec2<f32>)->vec2<f32> {
    return textureSample(plane,plane_sampler,uv).rg;
}
