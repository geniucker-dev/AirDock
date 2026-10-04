# SPDX-License-Identifier: MPL-2.0
"""Independent wire/media checks; no hardware parity claim is made by this test."""
import re
import argparse,hashlib,importlib.util,json,os,pathlib,plistlib,socket,struct,subprocess,sys,time
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.ciphers import Cipher,algorithms,modes

ROOT=pathlib.Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('pair',ROOT/'tools/test_pair_verify.py');pair=importlib.util.module_from_spec(spec);spec.loader.exec_module(pair)

def run(*args):return subprocess.run([str(a) for a in args],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE).stdout
def wait_listener(port,process):
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        if process.poll() is not None:raise RuntimeError('Receiver exited before listening')
        try:
            with socket.create_connection(('127.0.0.1',port),timeout=.2):return
        except OSError:time.sleep(.1)
    raise TimeoutError('Receiver did not listen')

def handshake(connection):
    client=pair.ClientCrypto();pk=pair.get_server_identity_pub(connection)
    status,_,response=connection.rpc('POST','/pair-verify',b'\1\0\0\0'+client.x25519_pub+client.ed_pub);assert status==200
    shared=client.x25519_priv.exchange(pair.x25519.X25519PublicKey.from_public_bytes(response[:32]));key,iv=pair.derive_key_iv(shared)
    cipher=Cipher(algorithms.AES(key),modes.CTR(iv)).decryptor();signature=cipher.update(response[32:]);pair.ed25519.Ed25519PublicKey.from_public_bytes(pk).verify(signature,response[:32]+client.x25519_pub)
    signature=client.ed_priv.sign(client.x25519_pub+response[:32]);assert connection.rpc('POST','/pair-verify',b'\0'*4+cipher.update(signature))[0]==200
    message_hex,key_hex=(ROOT/'rust/tests/fixtures/fairplay-regression.txt').read_text().strip().split()
    message=bytearray.fromhex(message_hex);message[:12]=b'FPLY\x03\x01\x03\0\0\0\0\x98';first=bytearray(16);first[:12]=b'FPLY\x03\x01\x01\0\0\0\0\x04'
    assert connection.rpc('POST','/fp-setup',first)[0]==200;assert connection.rpc('POST','/fp-setup',message)[0]==200
    stream_key=hashlib.sha512(bytes.fromhex('2a7ef6c632626859b485d44cfe6bc3a6')+shared).digest()[:16]
    return bytes.fromhex(key_hex),stream_key

def ffmpeg_hex(text):
    chunks=[]
    for line in text.splitlines():
        if ':' in line:
            hex_bytes=line.split(':',1)[1].strip().split('  ',1)[0].replace(' ','')
            if hex_bytes:chunks.append(bytes.fromhex(hex_bytes))
    return b''.join(chunks)

def send_mirror(connection,key,paths,ident=123456,port=None,signed_id=True):
    if port is None:
        # Apple eight-byte plist integers preserve the ID's unsigned bit pattern.
        wire_id=ident-(1<<64) if signed_id and ident>=(1<<63) else ident
        body=plistlib.dumps({'streams':[{'type':110,'streamConnectionID':wire_id}]},fmt=plistlib.FMT_BINARY)
        status,_,body=connection.rpc('SETUP','/stream',body,{'Content-Type':'application/x-apple-binary-plist'});assert status==200
        port=plistlib.loads(body)['streams'][0]['dataPort']
    derive=lambda salt:hashlib.sha512((salt+str(ident)).encode()+key).digest()[:16]
    cipher=Cipher(algorithms.AES(derive('AirPlayStreamKey')),modes.CTR(derive('AirPlayStreamIV'))).encryptor()
    with socket.create_connection(('127.0.0.1',port),timeout=3) as mirror:
        def packet(kind,payload):
            header=bytearray(128);struct.pack_into('<I',header,0,len(payload));struct.pack_into('>H',header,4,kind)
            # Fragment every record independently from codec and AES boundaries.
            record=header+payload
            mirror.sendall(record[:37]);mirror.sendall(record[37:131]);mirror.sendall(record[131:])
        for path in paths:
            probe=json.loads(path.with_suffix('.json').read_text())
            config=ffmpeg_hex(probe['streams'][0]['extradata'])
            if probe['streams'][0]['codec_name']=='hevc':
                box=struct.pack('>I',len(config)+8)+b'hvcC'+config
                config=struct.pack('>I',86+len(box))+b'hvc1'+bytes(78)+box
            packet(0x100,config)
            for video in probe['packets']:
                packet(0x10 if 'K' in video['flags'] else 0,cipher.update(ffmpeg_hex(video['data'])))
                time.sleep(1/30)
    return port

def wire_media(port,output,paths):
    pair.PORT=port;runner=pair.Runner()
    for test in [pair.t1_happy_path,pair.t2_ios_headers_and_absolute_uri,pair.t3_bad_client_signature,pair.t4_concurrent_sessions,pair.t5_fp_setup_framing,pair.t6_media_handshake_shape]:test(runner)
    assert runner.failed==0
    time.sleep(.1)
    with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as control:
        control.bind(('127.0.0.1',0));control.settimeout(2)
        c=pair.Conn('127.0.0.1',port)
        try:
            status,_,body=c.rpc('GET','/info');assert status==200;info=plistlib.loads(body);assert len(info['pk'])==32;assert info['features']==0x4005a7ffee6
            encrypted,key=handshake(c)
            body=plistlib.dumps({'ekey':encrypted,'eiv':bytes(16),'timingPort':control.getsockname()[1],'name':'Fixture iPhone','streams':[{'type':96,'ct':4,'spf':480,'controlPort':control.getsockname()[1]}]},fmt=plistlib.FMT_BINARY)
            status,_,body=c.rpc('SETUP','/stream',body,{'Content-Type':'application/x-apple-binary-plist'});assert status==200,body
            allocated=plistlib.loads(body);assert allocated['timingPort']>0;assert allocated['streams'][0]['dataPort']>0
            packet,peer=control.recvfrom(128);assert packet[:2]==b'\x80\xd2';assert packet[24:32]!=bytes(8)
            reply=bytearray(packet);reply[1]=0xd3;control.sendto(reply,peer)
            fixture=(ROOT/'rust/tests/fixtures/eld-480.bin').read_bytes();offset=0
            for i in range(64):
                size=int.from_bytes(fixture[offset:offset+4],'big');plain=fixture[offset+4:offset+4+size];offset+=size+4
                block=size//16*16;encrypted=Cipher(algorithms.AES(key),modes.CBC(bytes(16))).encryptor().update(plain[:block])+plain[block:]
                packet=b'\x80\xe0'+struct.pack('>HI',((65530+i)&65535),((0xffffff00+i*480)&0xffffffff))+bytes(4)+encrypted
                control.sendto(packet,('127.0.0.1',allocated['streams'][0]['dataPort']));time.sleep(.004)
            send_mirror(c,key,paths)
            assert c.rpc('FLUSH','/stream',extra_headers={'RTP-Info':'seq=100;rtptime=0'})[0]==200
            assert c.rpc('SET_PARAMETER','/stream',b'volume: -12.0\r\n',{'Content-Type':'text/parameters'})[0]==200
            assert b'-12.000000' in c.rpc('GET_PARAMETER','/stream',b'volume\r\n')[2]
            # Real iOS may open its replacement session while the old control
            # connection is still alive. Require complete old-worker teardown.
            replacement=pair.Conn('127.0.0.1',port)
            try:
                ekey,_=handshake(replacement)
                request=plistlib.dumps({'ekey':ekey,'eiv':bytes(16),'timingPort':control.getsockname()[1],'streams':[{'type':96,'ct':4,'spf':480,'controlPort':control.getsockname()[1]}]},fmt=plistlib.FMT_BINARY)
                assert replacement.rpc('SETUP','/stream',request,{'Content-Type':'application/x-apple-binary-plist'})[0]==200
                assert replacement.rpc('TEARDOWN','/stream')[0]==200
            finally:replacement.close()
        finally:c.close()
    return runner.passed

def generate_media(output,hdr=False):
    import shutil,hashlib
    fixtures=ROOT/'rust/tests/fixtures/media'
    for name,digest in json.loads((fixtures/'sha256.json').read_text()).items():
        assert hashlib.sha256((fixtures/name).read_bytes()).hexdigest()==digest,name
    paths=[]
    names=['hevc-main10-sdr.mp4','hevc-main10-pq.mp4','hevc-main10-hlg.mp4','libx264-96x128.mp4'] if hdr else [codec+'-'+size+'.mp4' for codec,size in [('libx264','128x96'),('libx264','96x128'),('libx265','128x96'),('libx265','96x128')]]
    for name in names:
        source=fixtures/name;path=output/source.name
        shutil.copy2(source,path);shutil.copy2(source.with_suffix('.json'),path.with_suffix('.json'));paths.append(path)
    return paths

def media_files(binary,paths,verifier=None):
    example=verifier.resolve() if verifier else binary.parent/'examples'/('media_verify.exe' if os.name=='nt' else 'media_verify')
    verification=json.loads(run(example,*paths));assert verification['decoded_frames']==120
    return verification

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--gui',action='store_true');parser.add_argument('--hdr',action='store_true');parser.add_argument('--media-verifier',type=pathlib.Path,help='External test helper for validating a packaged receiver');args=parser.parse_args();binary=args.binary.resolve()
    fault=os.environ.get('AIRPLAY_GPU_TEST_LOSS_AFTER_MS');output=ROOT/'rust-validation'/(('gpu-loss' if fault else 'mirror-gui' if args.gui else 'mirror')+('-hdr' if args.hdr else ''));output.mkdir(parents=True,exist_ok=True);config=output/'config';config.mkdir(exist_ok=True)
    env=os.environ.copy();env['AIRPLAY_AUDIO_NULL']='1';env['RUST_LOG']='warn';port=7010
    paths=generate_media(output,args.hdr)
    (config/'settings.json').write_text(json.dumps({'vsync':False}))
    with (output/'receiver.log').open('w') as log:
        process=subprocess.Popen([str(binary),* ([] if args.gui else ['--headless']),'--port',str(port),'--config-dir',str(config),'--metrics',str(output/'metrics.json'),'--exit-after','8'],env=env,stdout=log,stderr=log)
        try:wait_listener(port,process);checks=wire_media(port,output,paths)
        finally:
            try:process.wait(timeout=10)
            except subprocess.TimeoutExpired:process.kill();process.wait();raise
    metrics=json.loads((output/'metrics.json').read_text());assert metrics['audio_errors']==0,metrics;assert metrics['audio_packets']==64,metrics;assert metrics['decoded_frames']==120,metrics
    if args.gui:assert metrics['presented_frames']>0 and metrics['p95_receive_to_present_us']>0,metrics
    if fault:
        text=(output/'receiver.log').read_text();assert 'Synthetic acceptance fault' in text and 'GPU device reconstructed' in text,text
        assert metrics['presented_frames']>=80,metrics
    verification=media_files(binary,paths,args.media_verifier)
    if args.hdr:
        assert 'YUV420P10LE' in verification['validated_pixel_formats'],verification
        assert {'Pq:800','Hlg:1000'}<=set(verification['colour_profiles']),verification
        assert 'Video display:' not in (output/'receiver.log').read_text(),(output/'receiver.log').read_text()
        if args.gui:
            assert metrics['ten_bit_uploaded_frames']>=60 and metrics['hdr_uploaded_frames']>=40,metrics
    result={'protocol_checks':checks,'media':verification,'metrics':metrics,'synthetic_gpu_loss_recovery':bool(fault),'hardware_parity':'pending'}
    (output/'results.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))

if __name__=='__main__':main()
