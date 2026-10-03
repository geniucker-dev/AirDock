/* Test-only differential oracle, using the unchanged bundled PlayFair code.
 * This file is never linked into the Rust receiver. */
#include "playfair.h"
#include <stdio.h>
#include <string.h>
static int decode(const char* in, unsigned char* out, int count) {
    for(int i=0;i<count;i++) {unsigned value;if(sscanf(in+2*i,"%2x",&value)!=1)return 0;out[i]=(unsigned char)value;}return 1;
}
int main(void) {
    char line[1024],message_hex[329],key_hex[145];
    while(fgets(line,sizeof(line),stdin)) {
        unsigned char message[164],key[72],output[16];
        if(sscanf(line,"%328s %144s",message_hex,key_hex)!=2||strlen(message_hex)!=328||strlen(key_hex)!=144)return 1;
        if(!decode(message_hex,message,164)||!decode(key_hex,key,72)||message[12]>3)return 1;
        playfair_decrypt(message,key,output);for(int i=0;i<16;i++)printf("%02x",output[i]);puts("");
    }return 0;
}
