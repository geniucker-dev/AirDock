struct Uniforms { r:vec4<f32>,g:vec4<f32>,b:vec4<f32>,options:vec4<f32>,crop:vec4<f32> }
@group(0) @binding(0) var y_plane:texture_2d<f32>;
@group(0) @binding(1) var u_plane:texture_2d<f32>;
@group(0) @binding(2) var v_plane:texture_2d<f32>;
@group(0) @binding(3) var plane_sampler:sampler;
@group(0) @binding(4) var<uniform> params:Uniforms;
struct Vertex { @builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Vertex {
    let points=array<vec2<f32>,6>(vec2(-1.,1.),vec2(-1.,-1.),vec2(1.,-1.),vec2(-1.,1.),vec2(1.,-1.),vec2(1.,1.));
    var out:Vertex;let p=points[i];out.position=vec4(p,0.,1.);out.uv=(p*vec2(0.5,-0.5)+vec2(0.5))*params.crop.xy+params.crop.zw;return out;
}
fn linear(c:vec3<f32>)->vec3<f32> {return select(c/12.92,pow((c+vec3(0.055))/1.055,vec3(2.4)),c>vec3(0.04045));}
@fragment fn fs_main(input:Vertex)->@location(0) vec4<f32> {
    let y=textureSample(y_plane,plane_sampler,input.uv).r;
    let uv=textureSample(u_plane,plane_sampler,input.uv).rg;
    let v=textureSample(v_plane,plane_sampler,input.uv).r;
    let sample=vec4(y,uv.r,select(v,uv.g,params.options.x>0.5),1.);
    var rgb=clamp(vec3(dot(params.r,sample),dot(params.g,sample),dot(params.b,sample)),vec3(0.),vec3(1.));
    if params.options.y>0.5 {rgb=linear(rgb);}
    return vec4(rgb,1.);
}
