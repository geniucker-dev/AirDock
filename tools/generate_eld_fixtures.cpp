// SPDX-License-Identifier: GPL-3.0-only
// Independent fixture generator. Requires libfdk-aac only when regenerating
// test data; the Rust receiver uses FFmpeg and never links this encoder.
#include <aacenc_lib.h>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <vector>
static void check(bool value) {if(!value){std::fprintf(stderr,"FDK fixture generation failed\n");std::exit(1);}}
int main(int argc,char** argv) {
    check(argc==3);int spf=std::atoi(argv[1]);check(spf==480||spf==512);
    HANDLE_AACENCODER encoder=nullptr;check(aacEncOpen(&encoder,0,2)==AACENC_OK);
    for(auto p: {std::pair<AACENC_PARAM,UINT>{AACENC_AOT,AOT_ER_AAC_ELD},
        {AACENC_SAMPLERATE,44100},{AACENC_CHANNELMODE,MODE_2},{AACENC_CHANNELORDER,1},
        {AACENC_BITRATE,128000},{AACENC_SBR_MODE,0},{AACENC_GRANULE_LENGTH,UINT(spf)},{AACENC_TRANSMUX,TT_MP4_RAW}})
        check(aacEncoder_SetParam(encoder,p.first,p.second)==AACENC_OK);
    check(aacEncEncode(encoder,nullptr,nullptr,nullptr,nullptr)==AACENC_OK);
    auto* file=std::fopen(argv[2],"wb");check(file!=nullptr);int written=0;
    for(int frame=0;written<64&&frame<72;frame++) {
        std::vector<int16_t> pcm(spf*2);
        for(int i=0;i<spf;i++) {pcm[2*i]=int16_t(8000*std::sin(2*3.141592653589793*440*(frame*spf+i)/44100));pcm[2*i+1]=int16_t(5000*std::sin(2*3.141592653589793*880*(frame*spf+i)/44100));}
        uint8_t output[4096];void* in=pcm.data();void* out=output;int iid=IN_AUDIO_DATA,oid=OUT_BITSTREAM_DATA;
        int isize=pcm.size()*2,osize=sizeof(output),ielem=2,oelem=1;
        AACENC_BufDesc input{1,&in,&iid,&isize,&ielem},result{1,&out,&oid,&osize,&oelem};
        AACENC_InArgs args{};args.numInSamples=pcm.size();AACENC_OutArgs info{};
        check(aacEncEncode(encoder,&input,&result,&args,&info)==AACENC_OK);
        if(info.numOutBytes>0) {uint32_t size=info.numOutBytes;unsigned char prefix[]={uint8_t(size>>24),uint8_t(size>>16),uint8_t(size>>8),uint8_t(size)};
            check(std::fwrite(prefix,1,4,file)==4);check(std::fwrite(output,1,size,file)==size);written++;}
    }
    check(written==64);std::fclose(file);aacEncClose(&encoder);
}
