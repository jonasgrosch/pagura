"""Exact one-record sfnt count repair, OpenType TableDirectory/TableRecord.

Independent program writers and literal rectangle area oracles; no engine.
Strict keeps the original untagged overlap error. Legacy Strict-first scan
cannot discover this repair; the public API tests request Repair explicitly.
"""
import json
import struct
from dataclasses import dataclass
from fractions import Fraction as F
from typing import Any
from .font import truetype, cff_font
from .model import Vector
from .pdf import document, stream

AREA = "font_directory"
REQUIREMENT = "font-sfnt-directory-count"

def sfnt(first,otto=False,cid=False):
 raw=truetype(cff=cff_font(cid=cid,conforming=True) if otto else None);n=int.from_bytes(raw[4:6],'big');tables={}
 for i in range(n):tag,_,at,size=struct.unpack_from('>4sIII',raw,12+16*i);tables[tag]=raw[at:at+size]
 order=[first]+[tag for tag in sorted(tables) if tag!=first];cursor=12+16*n;offsets={};body=b''
 for tag in order:
  offsets[tag]=cursor;part=tables[tag];padded=part+bytes(-len(part)%4);body+=padded;cursor+=len(padded)
 directory=b''.join(tag+struct.pack('>III',0,offsets[tag],len(tables[tag])) for tag in sorted(tables));out=raw[:12]+directory+body;assert min(offsets.values())==12+16*n;return out
PROGRAMS={'ttf':sfnt(b'head'),'cff-cid':sfnt(b'CFF ',True,True)}
NAME_CFF=sfnt(b'CFF ',True)
def pdf(font,kind):
 descriptor=b'<< /Type /FontDescriptor /FontName /DirectoryFixture /Flags 32 /FontBBox [0 0 300 300] /ItalicAngle 0 /Ascent 300 /Descent 0 /CapHeight 300 /StemV 80 '+(b'/FontFile2 7 0 R' if kind=='ttf' else b'/FontFile3 7 0 R')+b' >>'
 if kind=='ttf':dictionary=b'<< /Type /Font /Subtype /Type0 /BaseFont /DirectoryFixture /Encoding /Identity-H /DescendantFonts [6 0 R] /ToUnicode 8 0 R >>';desc=b'<< /Type /Font /Subtype /CIDFontType2 /BaseFont /DirectoryFixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 9 0 R /CIDToGIDMap /Identity /DW 500 /W [1 [600]] >>';text=b'<0001>';cmap=b'<0000><FFFF>\nendcodespacerange\n1 beginbfchar\n<0001><0041>'
 else:dictionary=b'<< /Type /Font /Subtype /Type1 /BaseFont /DirectoryFixture /FontDescriptor 9 0 R /FirstChar 65 /LastChar 65 /Widths [600] /Encoding << /Type /Encoding /Differences [65 /A] >> /ToUnicode 8 0 R >>';desc=b'null';text=b'(A)';cmap=b'<00><FF>\nendcodespacerange\n1 beginbfchar\n<41><0041>'
 if kind=='cff-cid':
  dictionary=b'<< /Type /Font /Subtype /Type0 /BaseFont /DirectoryFixture /Encoding /Identity-H /DescendantFonts [6 0 R] /ToUnicode 8 0 R >>';desc=b'<< /Type /Font /Subtype /CIDFontType0 /BaseFont /DirectoryFixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 9 0 R /DW 500 /W [17 [600]] >>';text=b'<0011>';cmap=b'<0000><FFFF>\nendcodespacerange\n1 beginbfchar\n<0011><0041>'
 contents=b'0 0 1 rg 2 2 60 60 re f\n0 g\nBT /F 100 Tf 1 0 0 1 8 12 Tm '+text+b' Tj 1 0 0 1 32 9 Tm '+text+b' Tj ET\n'
 return document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << /Font << /F 5 0 R >> >> /Contents 4 0 R >>',stream(contents),dictionary,desc,stream(font,b'/Length1 '+str(len(font)).encode() if kind=='ttf' else b'/Subtype /OpenType'),stream(b'/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def 1 begincodespacerange\n'+cmap+b'\nendbfchar endcmap CMapName currentdict /CMap defineresource pop end end'),descriptor])
def area(rect,x,y,s):
 x0,y0,x1,y1=[F(v)*s for v in rect];return max(F(0),min(F(x+1),x1)-max(F(x),x0))*max(F(0),min(F(y+1),y1)-max(F(y),y0))
def quant(v):return int(v+F(1,2))
def oracle(dpi):
 s=F(dpi,72);side=(64*dpi+71)//72;rgb=bytearray();white=bytearray();trans=bytearray()
 for py in range(side):
  y=64*s-1-py
  for x in range(side):
   blue=area((2,2,62,62),x,y,s);black=area((8,12,18,32),x,y,s)+area((32,9,42,29),x,y,s);assert 0<=black<=blue<=1
   rgb+=bytes([quant(255*(1-blue)),quant(255*(1-blue)),quant(255*(1-black))]);white+=rgb[-3:]+b'\xff';trans+=bytes([0,0,quant(255*(blue-black)),quant(255*blue)])
 return side,b'P6\n%d %d\n255\n'%(side,side)+rgb,bytes(white),bytes(trans)


def records():
 rows = []
 def save(name,kind,program,expected,strict,reason,**extra):
  rows.append(dict(name=name,kind=kind,program=program,expected_repair=expected,strict=strict,reason=reason,**extra))
 for kind,valid in PROGRAMS.items():
  count=int.from_bytes(valid[4:6],'big');wrong=valid[:4]+struct.pack('>H',count+1)+valid[6:];overlap=next(12+16*i+8 for i in range(count) if struct.unpack_from('>I',valid,12+16*i+12)[0] and struct.unpack_from('>I',valid,12+16*i+8)[0]<12+16*(count+1))
  save(kind+'-valid',kind,valid,'rendered',{'status':'rendered'},'Conforming baseline control; no new repair')
  save(kind+'-count-plus-one',kind,wrong,'rendered',{'status':'refused','kind':'Syntax','message':'sfnt table overlaps directory','buffer_position':overlap,'repair':None},'Only numTables bytes differ from valid program; all glyph/table/data bytes unchanged',repair_tags=['font.sfnt-directory-count'],repair_buffer_position=4)
  for fault in ['count-plus-two','first-offset-inside-record','duplicate-prefix-tag','wrong-first-payload-header','prefix-out-of-bounds','unknown-signature','truncated-extra-header']:
   b=bytearray(wrong)
   if fault=='count-plus-two':b[4:6]=struct.pack('>H',count+2)
   elif fault=='first-offset-inside-record':
    target=next(i for i in range(count) if struct.unpack_from('>I',b,12+16*i+8)[0]==12+16*count);struct.pack_into('>I',b,12+16*target+8,12+16*count-4)
   elif fault=='duplicate-prefix-tag':b[28:32]=b[12:16]
   elif fault=='wrong-first-payload-header':b[12+16*count]=255
   elif fault=='prefix-out-of-bounds':struct.pack_into('>I',b,12+16*(count-1)+12,len(b)+1)
   elif fault=='unknown-signature':b[:4]=b'ttcf'
   elif fault=='truncated-extra-header':b=b[:12+16*count+8]
   save(kind+'-'+fault,kind,bytes(b),'refused',{'status':'refused'},'Must decline count proof; no output and no new count repair',forbidden_repairs=['font.sfnt-directory-count'])
 # Name-keyed PDF controls remain independent accuracy cases; malformed baselineRepair
 # may use the existing generic substitute, and those pixels are not oracles.
 valid=NAME_CFF;count=int.from_bytes(valid[4:6],'big');wrong=valid[:4]+struct.pack('>H',count+1)+valid[6:]
 save('cff-valid','cff',valid,'rendered',{'status':'rendered'},'Conforming name-keyed baseline control')
 save('cff-count-plus-one','cff',wrong,'rendered',{'status':'refused','kind':'Syntax','message':'sfnt table overlaps directory','buffer_position':20,'repair':None},'Candidate must use embeddedglyphs; oldRepair generic substitution is baseline characterization only',repair_tags=['font.sfnt-directory-count'],repair_buffer_position=4,baseline_repair_characterization='Mayrenderexistinggenericsubstitution;neverappearanceoracle')
 return rows+counter_records()


@dataclass
class Entry:
 tag: bytes
 checksum: int
 start: int
 size: int

 def wire(self) -> tuple[bytes, int, int, int]:
  return self.tag,self.checksum,self.start,self.size


def unpack(b):
 n=int.from_bytes(b[4:6],'big');return n,[Entry(*struct.unpack_from('>4sIII',b,12+16*i)) for i in range(n)]
def serialize(b,rows,count=None):
 rows=sorted(rows,key=lambda r:r.tag);return b[:4]+struct.pack('>H',len(rows) if count is None else count)+b[6:12]+b''.join(struct.pack('>4sIII',*r.wire()) for r in rows)+b[12+16*len(rows):]
def make(kind,fault):
 valid=PROGRAMS[kind];n,rows=unpack(valid);boundary=12+16*n;b=bytearray(valid[:4]+struct.pack('>H',n+1)+valid[6:])
 if fault=='genuine-table-overlap':
  first=min(r.start for r in rows if r.size);r=next(r for r in reversed(rows) if r.size and r.start!=first);r.start=first;return serialize(bytes(b),rows,n+1)
 if fault=='valid-looking-extra-tag':b[boundary:boundary+4]=b'TEST'
 elif fault=='first-payload-gap':
  for r in rows:
   if r.start>=boundary:r.start+=4
  return serialize(bytes(b[:boundary]+bytes(4)+b[boundary:]),rows,n+1)
 elif fault.startswith('missing-') or fault.startswith('mixed-'):
  old,new={'missing-head':(b'head',b'xxxx'),'missing-glyf':(b'glyf',b'xxxx'),'missing-cff':(b'CFF ',b'XXXX'),'mixed-cff':(b'post',b'CFF '),'mixed-glyf':(b'post',b'glyf')}[fault]
  next(r for r in rows if r.tag==old).tag=new;return serialize(bytes(b),rows,n+1)
 elif fault=='cff2':
  tables={r.tag:valid[r.start:r.start+r.size] for r in rows};del tables[b'CFF '];tables[b'CFF2']=cff_font(cff2=True,conforming=True)
  body=b'';newrows=[];cursor=boundary
  for tag in [b'CFF2']+[t for t in sorted(tables) if t!=b'CFF2']:
   data=tables[tag];newrows.append(Entry(tag,0,cursor,len(data)));padded=data+bytes(-len(data)%4);body+=padded;cursor+=len(padded)
  return valid[:4]+struct.pack('>H',n+1)+valid[6:12]+b''.join(struct.pack('>4sIII',*r.wire()) for r in sorted(newrows,key=lambda r:r.tag))+body
 elif fault=='ttc-outer':
  for r in rows:r.start+=16
  inner=serialize(bytes(b),rows,n+1)
  return b'ttcf'+struct.pack('>III',0x10000,1,16)+inner
 return bytes(b)
def original_contract(b,repair,operation='font') -> dict[str, Any]:
 # Current-base TTC dispatch precedes sfnt parsing. These controls keep the
 # exact parent refusal; the standalone count proof never applies to TTC.
 base=0
 if b[:4]==b'ttcf':
  version=int.from_bytes(b[4:8],'big')
  if version not in (0x10000,0x20000):
   return dict(kind='Unsupported(Feature("font.format"))',message='unknown font collection version',position=4,repair=None,diagnostics=[])
  assert int.from_bytes(b[8:12],'big')==1
  base=int.from_bytes(b[12:16],'big')
  if operation=='render':
   # These independently written fonts contain no name table. PDF binding
   # requires a unique original FontName/BaseFont match, never face zero.
   return dict(kind='Syntax',message='font collection name not found',position=12,repair=None,diagnostics=[])
 n=int.from_bytes(b[base+4:base+6],'big');end=base+12+16*n;diags=[];seen=set()
 # The shared parent reader frames the complete record slice before walking
 # records, so truncated claimed headers keep its earlier exact refusal.
 if len(b)<end:return dict(kind='Syntax',message='truncated font data',position=base+12,repair=None,diagnostics=[])
 for i in range(n):
  tag,_,start,size=struct.unpack_from('>4sIII',b,base+12+16*i);at=base+12+16*i+8
  assert size<=256*1024**2
  if size and start<end:return dict(kind='Syntax',message='sfnt table overlaps directory',position=at,repair=None,diagnostics=diags)
  assert not size or start%4==0
  if start+size>len(b):
   if not repair:return dict(kind='Syntax',message='sfnt table exceeds available data',position=at,repair='font.table-bounds',diagnostics=[])
   if 'font.table-bounds' not in diags:diags.append('font.table-bounds')
  if tag in seen:return dict(kind='Syntax',message='duplicate sfnt table',position=12+16*(i+1),repair=None,diagnostics=diags)
  seen.add(tag)
 assert b[:4]==b'ttcf'
 return dict(kind='Syntax',message='unsupported sfnt version',position=0,repair=None,diagnostics=diags)
def counter_records():
 rows=[]
 for kind,faults in [('ttf',['genuine-table-overlap','valid-looking-extra-tag','first-payload-gap','missing-head','missing-glyf','mixed-cff','ttc-outer']),('cff-cid',['genuine-table-overlap','valid-looking-extra-tag','first-payload-gap','missing-cff','cff2','mixed-glyf','ttc-outer'])]:
  for fault in faults:
   program=make(kind,fault);contracts={}
   for mode in ['strict','repair']:
    c=original_contract(program,mode=='repair');pc=original_contract(program,mode=='repair','render');pc['object']=[7,0];pc['repair']=pc['repair'] or ('font.embedded-unusable' if mode=='strict' else None);contracts[mode]={'standalone':c,'pdf':pc}
   strict=dict(contracts['strict']['standalone']);strict['buffer_position']=strict.pop('position');strict['status']='refused'
   rows.append(dict(name=kind+'-'+fault,kind=kind,program=program,expected_repair='refused',strict=strict,reason='Independent directory ambiguity/required-program/outer-TTC countercontrol; original error and no count repair',forbidden_repairs=['font.sfnt-directory-count'],contracts=contracts))
 return rows


def cases():
 for dpi in (72,73,144):
  side,rgb,white,transparent=oracle(dpi)
  yield Vector(f"literal-{dpi}", {".ppm":rgb,".white.rgba":white,".transparent.rgba":transparent},
               (REQUIREMENT,),"Literal blue backdrop and two black rectangle glyphs; exact area at the CropBox transform",
               extra={"width":side,"height":side,"dpi":dpi,"tolerance":0})
 for row in records():
  program=row.pop("program")
  # A buffer parser and a PDF font loader have different existing refusal
  # envelopes. Keep both literal contracts rather than adding a Strict hint.
  row["strict_scope"]="standalone-font-program"
  row["pdf_strict"]=(dict(status="refused",kind="Syntax",message="sfnt table overlaps directory",
      object=[7,0],stream_position=row["strict"]["buffer_position"],repair="font.embedded-unusable")
      if row["name"].endswith("count-plus-one") else
      dict(status="rendered") if row["strict"]["status"]=="rendered" else dict(status="refused"))
  if "contracts" in row:row["pdf_strict"]=dict(row["contracts"]["strict"]["pdf"],status="refused")
  portable={}
  for mode in ['strict','repair']:
   if (mode=='repair' and row['expected_repair']=='rendered') or row['name'].endswith('-valid'):
    tags=row.get('repair_tags',[]) if mode=='repair' else []
    portable[mode]={'font':{'status':'rendered','diagnostics':tags},'render':{'status':'rendered','diagnostics':tags}}
   else:
    error=original_contract(program,mode=='repair');font=dict(error);font['object']=None;page=original_contract(program,mode=='repair','render');page['object']=[7,0];page['repair']=page['repair'] or ('font.embedded-unusable' if mode=='strict' else None)
    portable[mode]={'font':{'status':'refused','error':font,'diagnostics':error['diagnostics']},'render':{'status':'refused','error':page,'diagnostics':page['diagnostics']}}
  row['portable_contracts']=portable
  expected=("count 5\nadvance 1 600\noutline 1\nm 0 0\nl 100 0\nl 100 200\nl 0 200\nh\n"
            if row["expected_repair"]=="rendered" else "parse-error\n")
  yield Vector(row["name"], {".font":program,".pdf":pdf(program,row["kind"]),
                            ".expect":expected.encode(),".contract.json":(json.dumps(row,sort_keys=True)+"\n").encode()},
               (REQUIREMENT,),row["reason"],extra={"format":"truetype" if row["kind"]=="ttf" else "opentype",
               "explicit_repair_api":True,"legacy_strict_first_scan_discovery":False,
               "expected_repair_render":row["expected_repair"],"strict":row["strict"],
               "strict_scope":row["strict_scope"],"pdf_strict":row["pdf_strict"],"portable_contracts":portable})
