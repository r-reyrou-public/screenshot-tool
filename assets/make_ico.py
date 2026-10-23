# assets/icon.png から assets/icon.ico(従来型BMP形式の 16/32/48px)を作る。リポジトリのルートで実行する
# (PNG圧縮のアイコンだと、ウイルス対策の機械学習判定が誤検知しやすかったため BMP 形式にしている)
import struct
from PIL import Image
src=Image.open("assets/icon.png").convert("RGBA")
sizes=[16,32,48]
entries=[];datas=[]
for s in sizes:
    im=src.resize((s,s),Image.LANCZOS)
    px=im.load()
    xor=bytearray()
    for y in range(s-1,-1,-1):
        for x in range(s):
            r,g,b,a=px[x,y]; xor+=bytes((b,g,r,a))
    rowb=((s+31)//32)*4
    andm=bytes(rowb*s)
    hdr=struct.pack("<IiiHHIIiiII",40,s,s*2,1,32,0,len(xor)+len(andm),0,0,0,0)
    datas.append(hdr+bytes(xor)+andm)
off=6+16*len(sizes)
out=struct.pack("<HHH",0,1,len(sizes))
for s,d in zip(sizes,datas):
    out+=struct.pack("<BBBBHHII",s,s,0,0,1,32,len(d),off); off+=len(d)
out+=b"".join(datas)
open("assets/icon.ico","wb").write(out)
