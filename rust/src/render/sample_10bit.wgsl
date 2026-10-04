// SPDX-License-Identifier: MPL-2.0
fn texel(plane:texture_2d<u32>,point:vec2<i32>)->vec2<f32> {
    let size=vec2<i32>(textureDimensions(plane));
    let word=textureLoad(plane,clamp(point,vec2(0),size-vec2(1)),0).rg;
    // Mask unused bits before interpolation. P010 stores MSB-aligned samples;
    // planar 10-bit stores LSB-aligned samples. No optional GPU format feature.
    if params.options.w>0.5 {return vec2<f32>(word>>vec2(6u));}
    return vec2<f32>(word&vec2(1023u));
}
fn sample_plane(plane:texture_2d<u32>,uv:vec2<f32>)->vec2<f32> {
    let position=uv*vec2<f32>(textureDimensions(plane))-vec2(0.5);
    let point=vec2<i32>(floor(position));
    let fraction=fract(position);
    return mix(mix(texel(plane,point),texel(plane,point+vec2(1,0)),fraction.x),
               mix(texel(plane,point+vec2(0,1)),texel(plane,point+vec2(1,1)),fraction.x),fraction.y);
}
