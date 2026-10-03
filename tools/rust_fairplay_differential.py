"""Compare all four modes against the unchanged C implementation, never linked to the application."""
import argparse, random, subprocess

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--reference',required=True);parser.add_argument('--rust',required=True);args=parser.parse_args()
    rng=random.Random(562120)
    vectors=[]
    for mode in range(4):
        for _ in range(64):
            message=bytearray(rng.randbytes(164));message[12]=mode;key=rng.randbytes(72)
            vectors.append(message.hex()+' '+key.hex())
    data=('\n'.join(vectors)+'\n').encode()
    reference=subprocess.run([args.reference],input=data,stdout=subprocess.PIPE,check=True).stdout.splitlines()
    rust=subprocess.run([args.rust],input=data,stdout=subprocess.PIPE,check=True).stdout.splitlines()
    assert len(reference)==256 and reference==rust, 'FairPlay differs from the existing implementation'
    print('PASS: 256 independent FairPlay vectors, all four modes')

if __name__=='__main__':main()
