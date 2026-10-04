struct Uniforms {
    r:vec4<f32>,g:vec4<f32>,b:vec4<f32>,options:vec4<f32>,crop:vec4<f32>,
    gamut_r:vec4<f32>,gamut_g:vec4<f32>,gamut_b:vec4<f32>,hdr:vec4<f32>
}
@group(0) @binding(0) var y_plane:texture_2d<PLANE_TYPE>;
@group(0) @binding(1) var u_plane:texture_2d<PLANE_TYPE>;
@group(0) @binding(2) var v_plane:texture_2d<PLANE_TYPE>;
@group(0) @binding(3) var plane_sampler:sampler;
@group(0) @binding(4) var<uniform> params:Uniforms;
@group(0) @binding(5) var color_lut:texture_2d<f32>;
// PLANE_SAMPLE
struct Vertex { @builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Vertex {
    let points=array<vec2<f32>,6>(vec2(-1.,1.),vec2(-1.,-1.),vec2(1.,-1.),vec2(-1.,1.),vec2(1.,-1.),vec2(1.,1.));
    var out:Vertex;let p=points[i];out.position=vec4(p,0.,1.);out.uv=(p*vec2(0.5,-0.5)+vec2(0.5))*params.crop.xy+params.crop.zw;return out;
}
fn linear(c:vec3<f32>)->vec3<f32> {return select(c/12.92,pow((c+vec3(0.055))/1.055,vec3(2.4)),c>vec3(0.04045));}
fn encoded(c:vec3<f32>)->vec3<f32> {
    return select(c*12.92,1.055*pow(c,vec3(1./2.4))-vec3(0.055),c>vec3(0.0031308));
}
fn table(x:f32,row:i32)->f32 {
    let size=i32(textureDimensions(color_lut).x);
    let p=clamp(x,0.,1.)*f32(size-1);
    let lo=i32(floor(p));
    return mix(textureLoad(color_lut,vec2(lo,row),0).r,
               textureLoad(color_lut,vec2(min(lo+1,size-1),row),0).r,fract(p));
}
fn colour_map(signal:vec3<f32>)->vec3<f32> {
    var rgb=vec3(table(signal.r,0),table(signal.g,0),table(signal.b,0));
    if params.hdr.x<2.5 {
        let weights=vec3(params.gamut_r.w,params.gamut_g.w,params.gamut_b.w);
        var lum=dot(rgb,weights);
        if params.hdr.x>1.5 {
            // BT.2100 HLG OOTF: scene-referred RGB -> 1000-nit display light.
            rgb*=1000.*pow(max(lum,0.),0.2);
            lum=dot(rgb,weights);
        }
        let mapped=table(lum/params.hdr.y,1);
        rgb*=mapped/max(lum,0.000001);
    }
    rgb=vec3(dot(params.gamut_r.xyz,rgb),dot(params.gamut_g.xyz,rgb),dot(params.gamut_b.xyz,rgb));
    // Compress chroma toward the neutral axis, preserving luminance and hue.
    let grey=clamp(dot(rgb,vec3(0.2126,0.7152,0.0722)),0.,1.);
    let delta=rgb-vec3(grey);
    var factor=1.;
    for(var i=0u;i<3u;i++) {
        if delta[i]>0.000001 {factor=min(factor,(1.-grey)/delta[i]);}
        if delta[i]<-0.000001 {factor=min(factor,-grey/delta[i]);}
    }
    return clamp(vec3(grey)+factor*delta,vec3(0.),vec3(1.));
}
@fragment fn fs_main(input:Vertex)->@location(0) vec4<f32> {
    let y=sample_plane(y_plane,input.uv).r;
    let uv=sample_plane(u_plane,input.uv);
    var v=uv.g;
    if params.options.x<0.5 {v=sample_plane(v_plane,input.uv).r;}
    let sample=vec4(vec3(y,uv.r,v)*params.options.z,1.);
    var rgb=clamp(vec3(dot(params.r,sample),dot(params.g,sample),dot(params.b,sample)),vec3(0.),vec3(1.));
    if params.hdr.x>0.5 {
        rgb=colour_map(rgb);
        if params.options.y<0.5 {rgb=encoded(rgb);}
    } else if params.options.y>0.5 {rgb=linear(rgb);}
    return vec4(rgb,1.);
}
