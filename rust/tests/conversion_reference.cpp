// Test-only reference for the established C++ renderer's BGRA conversion path.
// Never linked into the Rust application. Both processes use the same FFmpeg.
extern "C" {
#include <libavutil/frame.h>
#include <libavutil/pixfmt.h>
#include <libswscale/swscale.h>
}
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
int main(int argc, char** argv) {
    const int width = argc > 1 ? std::atoi(argv[1]) : 1920;
    const int height = argc > 2 ? std::atoi(argv[2]) : 1080;
    const int count = argc > 3 ? std::atoi(argv[3]) : 1200;
    if(width <= 0 || height <= 0 || count <= 0) return 1;
    auto source = av_frame_alloc();
    source->format = AV_PIX_FMT_NV12; source->width = width; source->height = height;
    if(av_frame_get_buffer(source,32) < 0) return 1;
    for(int y=0;y<height;++y) std::memset(source->data[0]+y*source->linesize[0],100,width);
    for(int y=0;y<(height+1)/2;++y) std::memset(source->data[1]+y*source->linesize[1],128,width);
    auto output = av_frame_alloc();
    output->format=AV_PIX_FMT_BGRA; output->width=width; output->height=height;
    if(av_frame_get_buffer(output,32)<0) return 1;
    SwsContext* converter=nullptr;
    auto convert = [&](const AVFrame* frame) {
        converter = sws_getCachedContext(converter,width,height,AV_PIX_FMT_NV12,width,height,
            AV_PIX_FMT_BGRA,SWS_FAST_BILINEAR,nullptr,nullptr,nullptr);
        if(!converter) std::exit(1);
        auto coefficients=sws_getCoefficients(SWS_CS_ITU709);
        if(sws_setColorspaceDetails(converter,coefficients,1,coefficients,1,0,1<<16,1<<16)<0) std::exit(1);
        if(sws_scale(converter,frame->data,frame->linesize,0,height,output->data,output->linesize)!=height) std::exit(1);
    };
    for(int i=0;i<60;++i) convert(source);
    const auto start=std::chrono::steady_clock::now();
    for(int i=0;i<count;++i) {auto frame=av_frame_clone(source);if(!frame)return 1;convert(frame);av_frame_free(&frame);}
    const double seconds=std::chrono::duration<double>(std::chrono::steady_clock::now()-start).count();
    std::printf("{\"width\":%d,\"height\":%d,\"frames\":%d,\"seconds\":%.9f,\"conversion_frames_per_second\":%.6f}\n",width,height,count,seconds,count/seconds);
    sws_freeContext(converter);av_frame_free(&source);av_frame_free(&output);
}
