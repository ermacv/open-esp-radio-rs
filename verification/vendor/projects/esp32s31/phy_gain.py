"""Native captured Wi-Fi/BT gain arithmetic and complete publication comparison.

Explicit finite software inputs only; no RF, protocol or whole-TXCAL qualification.
Private inputs and retained requests/evidence belong in the selected ignored output.
"""
import argparse
import copy
import hashlib
import itertools
import json
import pathlib
import shutil
import subprocess
import tempfile
import struct

from harness import Runner, ProbeCatalog, Buffer, seed, region, invocation, selection, case, words, symbol
from phy_i2c import LIBRARY_SHA, ROM_SHA

OBJECT_SHA = "88ee26018604100c9ba7839214024d54b48adf982c482dfb1e90d2a11f29f7d3"
# Independently extracted with llvm-ar/llvm-objcopy from the pinned archive;
# the native export must match before these data may support a comparison.
COEFFICIENT_SHA = "748936005c8ba31c1b826d0d0d2bb56f8982548c7d9826fed90930100d79f7a1"
INPUT, OUTPUT, ABI = 0x3fff0000, 0x3fff1000, 0x3fff4000
CURVES = [[0]*6, [0,5,10,15,20,25], [127,128,255,1,2,3], [255,0,127,128,7,9]]


def signed8(value):
    return (value & 127) - (value & 128)


def output(records, index, side=False):
    chunks = [r['chunk'] for r in records if r['kind'] == 'final-memory'
              and r['case'] == index and r['replacement'] == side]
    assert chunks and all(c['selection']==0 and c['offset']==sum(p['length'] for p in chunks[:i])
                          for i,c in enumerate(chunks))
    assert all(c['known'] == c['available'] == (1 << c['length'])-1 for c in chunks)
    return bytes(b for c in chunks for b in c['bytes'][:c['length']])


def events(records, index, side=False):
    return [r['event'] for r in records if r['kind'] == 'event'
            and r['case'] == index and r['replacement'] == side]


def calls(observations, target):
    found = []
    for index, e in enumerate(observations):
        if e['kind'] == 'call-transfer' and e['target'] == target:
            assert e['target_kind'] == 'captured-code'
            args = observations[index+1:index+1+e['words']]
            assert len(args)==e['words'] and all(a['kind'] == 'transfer-argument' and a['value']['kind'] == 'known' for a in args), args
            found.append([a['value']['value'] for a in args])
    return found


def arithmetic(coefficients, curve, base, correction, channel=None):
    """Independent interval oracle: choose each interval without cursor state."""
    low, mid, high = [struct.unpack('<18H',coefficients[i:i+36]) for i in (0,36,72)]
    high = [v if v < 32768 else v-65536 for v in high]
    if channel is None:
        targets = [base-correction+max(-72,min(80,-96+12*i)) for i in range(16)]
        interpolation = signed8(curve[1])
    else:
        if channel <= 6:
            a,b,position = signed8(curve[0]),signed8(curve[1]),channel-1
            interpolation = signed8(int((b-a)*position/5)+a)
        elif channel <= 11:
            a,b,position = signed8(curve[1]),signed8(curve[2]),channel-6
            interpolation = signed8(int((b-a)*position/5)+a)
        else:
            interpolation = signed8(signed8(curve[2])+2)
        targets = [base-correction+84-4*i for i in range(32)]
    indexes = [next((i for i,h in enumerate(high) if target >= h),17) for target in targets]
    residuals = [t-high[i]-interpolation for t,i in zip(targets,indexes)]
    if channel is None:
        residuals = [max(-60,min(24,v)) for v in residuals]
    return bytes(v&255 for v in residuals) + b''.join(struct.pack('<H',mid[i]) for i in indexes) + b''.join(struct.pack('<H',low[i]) for i in indexes)


def gain_models(base, fill):
    return [dict(id='gain-base', applicability='explicit gain bank base', lifetime='phase',
        behavior=dict(kind='constant-read',address=0x20100408,width=4,value=(base<<24)|0x005aa55a)),
        dict(id='gain-ports',applicability='retained index and three passive data ports; no gain algorithm',lifetime='phase',
        behavior=dict(kind='register-bank',cells=[dict(address=a,width=4,value=fill*0x01010101 if a==0x20100844 else 0)
                      for a in range(0x20100844,0x20100854,4)]))]


def publication(seed_words, config, calculated, count, base, fill, bluetooth=False):
    """Instruction-derived packed-word oracle, independent of the device model."""
    bb = struct.unpack('<'+'H'*count,calculated[count:count*3])
    rf = struct.unpack('<'+'H'*count,calculated[count*3:count*5])
    seed_data = bytes(words(seed_words))
    result = [('read',0x20100408,(base<<24)|0x005aa55a)]
    control = fill*0x01010101
    for i in range(count):
        index = {0:0,128:1,256:2}[bb[i]]
        a,b,c,d = struct.unpack_from('<4H',seed_data,index*8)
        word0 = ((c<<22)|(b<<31)|(d<<13)|(config&8191))&0xffffffff
        word1 = ((a<<8)|(b>>1)|(((bb[i]>>6)&255)<<17)|((rf[i]&7)<<31)|((bb[i]&63)<<20)|0x10000000)&0xffffffff
        word2 = ((rf[i]>>1)&31)|(calculated[i]<<15)|0x7f80
        slot = (base+(32 if bluetooth else 0)+i)&255
        # The captured ROM publisher writes data first, then updates its index.
        result += [('write',0x20100848,word0),('write',0x2010084c,word1),('write',0x20100850,word2),
                   ('read',0x20100844,control)]
        control = (control&0xfff00000)|0x80000|(slot<<11)
        result += [('write',0x20100844,control)]
    return result


class Gain:
    def __init__(self, options):
        options.output.mkdir(parents=True, exist_ok=True)
        self.run = pathlib.Path(tempfile.mkdtemp(prefix='run-', dir=options.output.resolve()))
        (options.output / 'latest').write_text(str(self.run))
        self.runner = Runner(options.binary, self.run, self.run/'project', options.limit_mode)
        self.call, self.doc = self.runner.call, self.runner.doc
        self.revision, identities = self.runner.capture(
            [options.library, options.rom, options.production], ['phy','rom','production'],
            [LIBRARY_SHA, ROM_SHA, None])
        self.doc('identities', dict(sha256=identities, scope=__doc__))
        self.inventory = self.call('inventory',['inventory'])['snapshot']['revision']['inputs']
        self.probes = ProbeCatalog.capture(self.runner, self.revision, self.inventory, 2)
        obj = next(o for o in self.inventory[0]['inventory']['objects'] if bytes(o['name']) == b'phy_tx_gain.o')
        section = next(s for s in obj['elf']['sections'] if bytes(s['name']) == b'.rodata')
        request = dict(occurrence=dict(revision=self.revision, source=dict(kind='input',input=0),
            object=obj['id'],symbol=None), analyses=[], ranges=[dict(kind='section', section=section['index'],offset=0,length=216)])
        self.call('coefficients',['data','--request',self.doc('coefficients',request),'--output',self.run/'coefficients'])
        assert hashlib.sha256((self.run/'coefficients/object.elf').read_bytes()).hexdigest() == OBJECT_SHA
        self.coefficients = (self.run/'coefficients/data.bin').read_bytes()
        assert hashlib.sha256(self.coefficients).hexdigest() == COEFFICIENT_SHA
        roots = ['phy_wifi_get_tx_tab_new','phy_bt_get_tx_tab_new','phy_set_tx_gain_mem_new','phy_bt_set_tx_gain_new','phy_get_romfunc_addr']
        companions = ['memcpy','phy_wifi_get_tx_gain','phy_bt_get_tx_gain','phy_txbbgain_to_index','phy_write_gain_mem','phy_param_addr','phy_get_romfuncs','phy_get_data_sat']
        # Co-located archive sections retain physical references outside the
        # selected gain roots. Bind these names to captured ROM definitions;
        # the run never grants their absent hardware inputs or executes a model.
        companions += ['phy_i2c_writeReg','memset','phy_get_i2c_mst0_mask','phy_i2c_paral_write_num',
                       'ets_delay_us','phy_wait_i2c_sdm_stable','phy_tsens_dac_cal','phy_tsens_temp_read_local',
                       'phy_i2c_readReg','phy_i2c_writeReg_Mask']
        link = dict(revision=self.revision,inputs=[0],entry=self.root(roots[0]),roots=[self.root(n) for n in roots[1:]],
            companions=[dict(input=1,symbol=self.sym(1,n)['id']) for n in companions],
            layout=dict(code=dict(start=0x11000000,length=0x1000000),data=dict(start=0x20000000,length=0x1000000)))
        plan = self.run/'link-plan.json'
        self.call('plan',['link-plan','--request',self.doc('plan',link),'--linker',options.linker.resolve(),'--output',plan])
        image = self.call('prepare',['prepare-image','--plan',plan,'--linker',options.linker.resolve()])['run']['image']
        view = self.call('image',['image','--id',image])
        self.roots = {bytes(r['name']).decode():r['address'] for r in view['summary']['manifest']['roots']}
        self.roots[roots[0]] = view['summary']['manifest']['entry']
        self.call('export-image',['export-image','--id',image,'--output',self.run/'image'])
        nm = subprocess.check_output([str(options.nm.resolve()),'--defined-only','--format=posix',str(self.run/'image/image.elf')]).decode()
        (self.run/'image-symbols.txt').write_text(nm)
        self.symbols = {}
        for line in nm.splitlines():
            parts = line.split()
            self.symbols.setdefault(parts[0], []).append(int(parts[2],16))
        self.parameter = self.address('phy_param')
        assert next(int(line.split()[3],16) for line in nm.splitlines() if line.split()[0]=='phy_param') == 516
        self.image_object = dict(artifact=view['summary']['manifest']['elf'],location=dict(kind='standalone'))
        self.vendor = dict(revision=self.revision,source=dict(kind='image',image=image),companions=[1,2],
                           abi='riscv-integer',stack=seed(0x3ffe0000,0x8000))
        self.production = dict(revision=self.revision,source=dict(kind='input',input=2),companions=[1],
                               abi='riscv-integer',stack=seed(0x3ffe0000,0x8000))
        self.artifacts = []

    def sym(self, index, name):
        return symbol(self.inventory,index,name)

    def root(self,name):
        return dict(input=0,symbol=self.sym(0,name)['id'])

    def address(self,name):
        matches = self.symbols[name]
        assert len(matches) == 1, name
        return matches[0]

    def enter(self, target, args=(), memory=(), observe=(), models=()):
        return invocation(self.probes.entry('open_phy_trace_stack_entry')['value'], [target,ABI],
            [region(ABI,64,words(args,pad_to=16,fill=0))]+list(memory),models,observe)

    def setup(self, data, production=False):
        return self.enter(self.sym(1,'memcpy')['value'],
            [INPUT+0x800 if production else self.parameter, INPUT,516],
            [region(INPUT,516,data),region(INPUT+0x800,516,lifetime='session')] if production else [region(INPUT,516,data)])

    def execute(self,label,rows,fill,production=True,verdict='MATCH',maximum=32768,right_target=None):
        if not production:
            rows = copy.deepcopy(rows)
            for row in rows:
                row['relation'] = None
        left = copy.deepcopy(self.vendor)
        right = copy.deepcopy(right_target or self.production) if production else None
        left['stack']['fill'] = fill
        if right: right['stack']['fill'] = fill
        request = dict(schema=17,vendor=left,replacement=right,binding='shared-core' if right else None,
                       cases=rows,max_events=maximum)
        identity = self.call(label,['compare' if right else 'execute','--request',self.doc(label,request)])['run']['execution']
        evidence = self.call(label+'-evidence',['execution','--id',identity])
        assert evidence['summary']['manifest']['verdict'] == (verdict if production else None), label
        records = [r['value'] for r in evidence['records']]
        outcomes = [r for r in records if r['kind']=='outcome']
        assert len(outcomes)==len(rows)*(2 if right else 1), label
        if verdict in ('MATCH','DIFF'):
            assert evidence['summary']['manifest']['complete'], label
            assert all(r['stop']['kind'] == 'returned' for r in outcomes), (label,outcomes)
        self.artifacts.append((label,identity,evidence))
        return records,request,identity

    def wifi_phase(self,channel,fill, capture=False):
        v = self.enter(self.roots['phy_wifi_get_tx_tab_new'],[channel,OUTPUT,OUTPUT+32,OUTPUT+96,0],
                       [region(OUTPUT,160,fill=fill)], [selection(OUTPUT,160)])
        if capture:
            v['observe_calls'] = dict(include_tail=True,argument_words=0,overrides=[
                dict(target=self.sym(1,'memcpy')['value'],words=3),
                dict(target=self.sym(1,'phy_wifi_get_tx_gain')['value'],words=11)])
        return v

    def wifi(self):
        for fill in (0x5a,0xa5):
            specs = list(itertools.product(CURVES,[1,2,5,6,7,10,11,12,13],[(0,0),(-128,127),(127,-128),(31,-17),(-31,17)]))
            for batch in range(0,len(specs),6):
                rows = []
                for curve,channel,(base,correction) in specs[batch:batch+6]:
                    data = [0]*516
                    data[241:247],data[247],data[291] = curve,correction&255,base&255
                    rows.append(case('initialize',self.setup(data),self.setup(data,True)))
                    right = self.probes.invoke('open_phy_channel_trace_calculate_tx_gain',dict(channel=channel,
                        curve=Buffer(INPUT,curve),correction=correction,base_and_delta=base,output=Buffer(OUTPUT,fill=fill)),
                        observe=[selection(OUTPUT,160)])
                    right = self.enter(right['entry'],right['arguments'],right['memory'],right['observe_memory'])
                    rows.append(case(f'wifi-{curve}-{channel}-{base}-{correction}',self.wifi_phase(channel,fill),right,reset='warm',memory=True))
                records,request,_ = self.execute(f'wifi-{fill}-{batch}',rows,fill)
                if not hasattr(self,'wifi_request'):
                    self.wifi_request = request
                assert not any(r['kind']=='event' for r in records)
                for i in range(1,len(rows),2):
                    curve,channel,(base,correction) = specs[batch+(i-1)//2]
                    expected = arithmetic(self.coefficients[108:],curve,base,correction,channel)
                    assert output(records,i) == output(records,i,True) == expected, (batch,i)

    def coefficient_boundaries(self):
        # The legacy input matrix leaves some table intervals unselected in the
        # production path. Select every authenticated threshold exactly, without
        # reading private production constants or supplying vendor results.
        for bluetooth in (False,True):
            coefficients = self.coefficients[:108] if bluetooth else self.coefficients[108:]
            high = struct.unpack('<18h',coefficients[72:])
            length = 80 if bluetooth else 160
            for fill in (0x5a,0xa5):
                for batch in range(0,18,6):
                    rows,expected = [],[]
                    for index in range(batch,batch+6):
                        data = [0]*516
                        if bluetooth:
                            base,correction = 64,-8-high[index]
                            data[292],data[254] = base,correction&255
                            left = self.enter(self.roots['phy_bt_get_tx_tab_new'],[OUTPUT+48,OUTPUT+16,OUTPUT,0],
                                [region(OUTPUT,80,fill=fill)],[selection(OUTPUT,80)])
                            packed = [0]*7+[(correction&255)<<24,base]
                            right = self.probes.invoke('open_phy_bluetooth_trace_calculate_gain',dict(
                                input=Buffer(INPUT,words(packed)),output=Buffer(OUTPUT,fill=fill)),observe=[selection(OUTPUT,80)])
                            expected.append(arithmetic(coefficients,[0]*3,base,correction))
                        else:
                            base,correction = -64,20-high[index]
                            data[291],data[247] = base&255,correction&255
                            left = self.wifi_phase(1,fill)
                            right = self.probes.invoke('open_phy_channel_trace_calculate_tx_gain',dict(channel=1,
                                curve=Buffer(INPUT,[0]*6),correction=correction,base_and_delta=base,output=Buffer(OUTPUT,fill=fill)),
                                observe=[selection(OUTPUT,160)])
                            expected.append(arithmetic(coefficients,[0]*6,base,correction,1))
                        assert -128 <= correction <= 127
                        right = self.enter(right['entry'],right['arguments'],right['memory'],right['observe_memory'])
                        rows += [case('initialize',self.setup(data),self.setup(data,True)),
                                 case(f'coefficient-interval-{index}',left,right,reset='warm',memory=True)]
                    label = f'coefficient-boundary-{bluetooth}-{fill}-{batch}'
                    records,_,_ = self.execute(label,rows,fill)
                    assert not any(r['kind']=='event' for r in records)
                    for i,value in enumerate(expected):
                        assert len(value)==length and output(records,2*i+1)==output(records,2*i+1,True)==value

    def characterize(self):
        for fill in (0x5a,0xa5):
            specs = list(itertools.product([CURVES[0],CURVES[2],CURVES[3]], [1,6,11,12,13],
                [(0,0,0),(127,1,127),(128,255,-128),(255,1,-17),(0,255,17)]))
            for batch in range(0,len(specs),4):
                rows = []
                for curve,channel,(base,adjustment,correction) in specs[batch:batch+4]:
                    data = [0]*516
                    data[241:247],data[247],data[291],data[434] = curve,correction&255,base,adjustment
                    rows += [case('initialize',self.setup(data),None),
                             case('current-kernel-inputs',self.wifi_phase(channel,fill,True),None,reset='warm')]
                records,_,_ = self.execute(f'kernel-inputs-{fill}-{batch}',rows,fill,False)
                direct_rows = []
                for i,(curve,channel,(base,adjustment,correction)) in enumerate(specs[batch:batch+4]):
                    ev = events(records,2*i+1)
                    assert all(e['kind'] in ('call-transfer','transfer-argument') for e in ev)
                    kernel = calls(ev,self.sym(1,'phy_wifi_get_tx_gain')['value'])
                    effective = signed8((base+adjustment)&255)
                    assert len(kernel) == 1 and kernel[0][:4] == [channel,self.parameter+241,correction&0xffffffff,effective&0xffffffff]
                    copies = calls(ev,self.sym(1,'memcpy')['value'])
                    assert len(copies) == 3 and all(c[2] == 36 for c in copies)
                    sources = [c[1] for c in copies]
                    assert sources == [sources[0]+i*36 for i in range(3)]
                    if not hasattr(self,'coefficient_address'):
                        self.coefficient_address = sources[0]-108
                        request = dict(occurrence=dict(revision=self.revision, source=self.vendor['source'],
                            object=self.image_object,symbol=None),analyses=[],
                            ranges=[dict(kind='image',address=self.coefficient_address,length=216)])
                        self.call('linked-coefficients',['data','--request',self.doc('linked-coefficients',request),
                            '--output',self.run/'linked-coefficients'])
                        assert (self.run/'linked-coefficients/data.bin').read_bytes() == self.coefficients
                    assert sources[0] == self.coefficient_address+108
                    assert kernel[0][4:7] == [c[0] for c in copies]
                    assert kernel[0][7:] == [OUTPUT,OUTPUT+32,OUTPUT+96,0]
                    data = [0]*516
                    data[241:247],data[247],data[291],data[434] = curve,correction&255,base,adjustment
                    direct = self.enter(self.sym(1,'phy_wifi_get_tx_gain')['value'],
                        [channel,self.parameter+241,correction&0xffffffff,effective&0xffffffff]+sources+[OUTPUT,OUTPUT+32,OUTPUT+96,0],
                        [region(OUTPUT,160,fill=fill)],[selection(OUTPUT,160)])
                    direct_rows += [case('initialize',self.setup(data),None),case('direct-ROM-kernel',direct,None,reset='warm')]
                direct,_,_ = self.execute(f'kernel-direct-{fill}-{batch}',direct_rows,fill,False)
                assert not any(r['kind']=='event' for r in direct)
                for i,(curve,channel,(base,adjustment,correction)) in enumerate(specs[batch:batch+4]):
                    expected = arithmetic(self.coefficients[108:],curve,signed8((base+adjustment)&255),correction,channel)
                    assert output(records,2*i+1) == output(direct,2*i+1) == expected

    def publish_wifi(self):
        for fill in (0x5a,0xa5):
            rows,expectations = [],[]
            for seed_value,base in itertools.product([0,0x13572468,0xffffffff],[0,32,224,255]):
                image = [(seed_value+i*0x01020305)&0xffffffff for i in range(47)]
                for i in range(32):
                    shift = (i%2)*16
                    image[14+i//2] = (image[14+i//2]&~(65535<<shift))|([0,128,256][i%3]<<shift)
                left = self.enter(self.roots['phy_set_tx_gain_mem_new'],[0,32,INPUT+120,INPUT+56,INPUT+24,INPUT,INPUT+184],
                    [region(INPUT,188,words(image))],models=gain_models(base,fill))
                right = self.probes.invoke('open_phy_channel_trace_publish_tx_gain',dict(input=Buffer(INPUT,words(image))),models=gain_models(base,fill))
                right = self.enter(right['entry'],right['arguments'],right['memory'],models=right['models'])
                rows.append(case(f'publish-{seed_value}-{base}',left,right))
                expectations.append(publication(image[:6],image[46],bytes(words(image[6:46])),32,base,fill))
            records,request,_ = self.execute(f'publish-wifi-{fill}',rows,fill)
            self.publication_request = request
            assert all(r['stop']['low']==0 for r in records if r['kind']=='outcome' and r['replacement'])
            for i,expected in enumerate(expectations):
                for side in (False,True):
                    ev = events(records,i,side)
                    assert [(e['kind'],e['address'],e['value']) for e in ev] == expected, (i,side,ev[:6],expected[:6])
                    assert len(ev) == 161 and all(e['width']==4 for e in ev)

    def bluetooth(self):
        for fill in (0x5a,0xa5):
            specs = list(itertools.product([[0,0,0],[127,128,255],[255,31,128]],
                [(0,0,0),(127,255,127),(128,1,-128),(255,31,-17),(0,127,17)], [0,32,224,255]))
            for batch in range(0,len(specs),3):
                rows,expected = [],[]
                for curve,(base,attenuation,correction),bank in specs[batch:batch+3]:
                    packed = [(fill*0x01010101+i*0x01020305)&0xffffffff for i in range(6)]
                    packed += [fill*0x0101,int.from_bytes(bytes(curve+[correction&255]),'little'),base|(attenuation<<8)]
                    data = [0]*516
                    data[260:284] = words(packed[:6])
                    data[208:210] = list(packed[6].to_bytes(2,'little'))
                    data[251:255] = words([packed[7]])
                    data[292],data[8],data[291],data[434] = base,attenuation,fill,fill
                    rows.append(case('initialize',self.setup(data),self.setup(data,True)))
                    install = self.enter(self.roots['phy_get_romfunc_addr'],memory=[
                        region(0x2f07fc3c,4,words([0x2f07f944]),lifetime='session'),
                        region(0x2f07fc40,4,lifetime='session')],
                        observe=[selection(0x2f07f96c,4)])
                    noop = self.enter(self.sym(1,'memcpy')['value'],[INPUT+0x800,INPUT+0x800,0])
                    rows.append(case('install-captured-callbacks',install,noop,reset='warm'))
                    left = self.enter(self.roots['phy_bt_get_tx_tab_new'],[OUTPUT+48,OUTPUT+16,OUTPUT,0],
                        [region(OUTPUT,80,fill=fill,lifetime='session')],[selection(OUTPUT,80)])
                    right = self.probes.invoke('open_phy_bluetooth_trace_calculate_gain',dict(
                        input=Buffer(INPUT,words(packed)),output=Buffer(OUTPUT,fill=fill,lifetime='session')),observe=[selection(OUTPUT,80)])
                    right = self.enter(right['entry'],right['arguments'],right['memory'],right['observe_memory'])
                    rows.append(case('bt-calculation',left,right,reset='warm',memory=True))
                    left = self.enter(self.roots['phy_bt_set_tx_gain_new'],[0],observe=[selection(OUTPUT,80)],models=gain_models(bank,fill))
                    left['observe_calls'] = dict(include_tail=True,argument_words=0,overrides=[])
                    right = self.probes.invoke('open_phy_bluetooth_trace_tx_gain',dict(input=Buffer(INPUT,words(packed)),output=OUTPUT),
                        observe=[selection(OUTPUT,80)],models=gain_models(bank,fill))
                    right = self.enter(right['entry'],right['arguments'],right['memory'],right['observe_memory'],right['models'])
                    rows.append(case('bt-complete-publication',left,right,reset='warm',memory=True))
                    calc = arithmetic(self.coefficients[:108],curve,signed8((base-attenuation)&255),correction)
                    expected.append((calc,publication(packed[:6],packed[6],calc,16,bank,fill,True)))
                records,request,_ = self.execute(f'bluetooth-{fill}-{batch}',rows,fill)
                if not hasattr(self,'bluetooth_request'):
                    self.bluetooth_request = request
                for i,(calc,writes) in enumerate(expected):
                    assert output(records,4*i+1) == self.roots['phy_bt_get_tx_tab_new'].to_bytes(4,'little')
                    assert any(e['kind']=='call-transfer' and e['target']==self.roots['phy_bt_get_tx_tab_new']
                               and e['indirect'] and e['target_kind']=='captured-code' for e in events(records,4*i+3))
                    for side in (False,True):
                        assert not events(records,4*i+2,side)
                        assert output(records,4*i+2,side) == output(records,4*i+3,side) == calc
                        ev = [e for e in events(records,4*i+3,side) if e['kind'] != 'call-transfer']
                        assert len(ev)==81 and all(e['width']==4 for e in ev)
                        assert [(e['kind'],e['address'],e['value']) for e in ev] == writes, (batch,i,side)

    def additive(self):
        profiles = [(0,0,0)]+[(0,a,0) for a in (1,31,127,255)]
        pairs = [(0,1),(0,31),(127,1),(255,1),(10,255)]
        for base,adjustment in pairs:
            profiles += [(base,0,adjustment),((base+adjustment)&255,0,0)]
        rows = []
        for base,attenuation,adjustment in profiles:
            data = [0]*516
            data[291],data[8],data[434] = base,attenuation,adjustment
            rows += [case('initialize',self.setup(data),None),case('additive-current',self.wifi_phase(13,0xa5),None,reset='warm')]
        records,_,_ = self.execute('additive-current',rows,0x5a,False)
        assert not any(r['kind']=='event' for r in records)
        baseline = output(records,1)
        assert all(output(records,i*2+1)==baseline for i in range(1,5))
        for i in range(5,15,2):
            assert output(records,i*2+1)==output(records,(i+1)*2+1)
        assert output(records,15)!=baseline  # (base=0, adjustment=31)
        old = []
        for base,attenuation,adjustment in [(0,0,0),(31,31,0),(0,0,31)]:
            data = [0]*516
            data[291],data[8],data[434] = base,attenuation,adjustment
            phase = self.enter(self.sym(1,'phy_wifi_get_tx_tab_')['value'],[13,OUTPUT,OUTPUT+32,OUTPUT+96,0],
                [region(self.sym(1,'phy_param_rom')['value'],4,words([INPUT+0x800])),region(OUTPUT,160,fill=0xa5)],
                [selection(OUTPUT,160)])
            old += [case('initialize',self.setup(data,True),None),case('subtractive-ROM',phase,None,reset='warm')]
        observed,_,_ = self.execute('subtractive-ROM',old,0x5a,False)
        assert not any(r['kind']=='event' for r in observed)
        assert output(observed,1)==output(observed,3)==output(observed,5)!=baseline

    def negative(self):
        unknown = copy.deepcopy(self.wifi_request)
        unknown['cases'] = unknown['cases'][:2]
        curve = unknown['cases'][1]['replacement']['memory'][1]['seed']
        assert curve['address']==INPUT
        curve['bytes'],curve['fill'] = [],None
        records,_,_ = self.execute('gain-unknown-curve',unknown['cases'],0x5a,verdict='INCOMPLETE')
        stop = next(r['stop'] for r in records if r['kind']=='outcome' and r['case']==1 and r['replacement'])
        assert stop['kind']=='incomplete' and stop['reason']['kind']=='memory',stop
        uninstalled = copy.deepcopy(self.bluetooth_request['cases'][:4])
        # Keep parameter setup and the output calculation, but do not execute
        # the installer. The archive's callback pointer remains unknown.
        del uninstalled[1]
        uninstalled[-1]['name'] = 'missing-callback-installation'
        records,_,_ = self.execute('gain-missing-callback',uninstalled,0x5a,verdict='INCOMPLETE')
        stop = next(r['stop'] for r in records if r['kind']=='outcome' and r['case']==2 and not r['replacement'])
        assert stop['kind']=='incomplete',stop
        assert not [e for e in events(records,2) if e['kind'] in ('write','read')]
        _,previous,previous_evidence = self.artifacts[-1]
        limited = copy.deepcopy(self.publication_request)
        limited['cases'] = limited['cases'][:1]
        limited['max_events'] = 1
        failed = self.call('gain-capacity',['compare','--request',self.doc('gain-capacity',limited)],expected=1)['run']
        assert failed.get('execution') is None and failed.get('publication') is None and failed.get('resolved_operation') is None
        assert failed['error']['code']=='resource-limited'
        retained = self.call('retained-after-failure',['execution','--id',previous])
        assert retained['records']==previous_evidence['records']
        assert retained['summary']['manifest']==previous_evidence['summary']['manifest']
        # The same ROM kernel receives caller-owned coefficient bytes. This is
        # explicitly a vendor-boundary characterization, not a production match.
        data = [0]*516
        direct = self.enter(self.sym(1,'phy_wifi_get_tx_gain')['value'],
            [13,INPUT+0x800+241,0,0,INPUT+256,INPUT+292,INPUT+328,OUTPUT,OUTPUT+32,OUTPUT+96,0],
            [region(INPUT+256,108,self.coefficients[108:]),region(OUTPUT,160,fill=0x5a)], [selection(OUTPUT,160)])
        rows = [case('initialize',self.setup(data),self.setup(data,True)),
                case('current-coefficients',self.wifi_phase(13,0x5a),direct,reset='warm',memory=True)]
        _,_,original = self.execute('gain-coefficients-original',rows,0x5a,right_target=self.vendor)
        changed = copy.deepcopy(rows)
        changed[1]['replacement']['memory'][1]['seed']['bytes'][72] ^= 1
        records,_,different = self.execute('gain-coefficients-changed',changed,0x5a,verdict='DIFF',right_target=self.vendor)
        assert different != original
        assert output(records,1) != output(records,1,True)

    def preserve(self):
        self.call('backup',['backup','--output',self.run/'backup.blobray'])
        shutil.move(self.runner.project,self.run/'moved')
        self.runner.project = self.run/'moved'
        label,identity,before = self.artifacts[0]
        moved = self.call('moved',['execution','--id',identity])
        assert moved['records'] == before['records']
        self.runner.project = self.run/'restored'
        self.call('restore',['restore','--backup',self.run/'backup.blobray'])
        for label,identity,before in self.artifacts:
            restored = self.call('restored-'+label,['execution','--id',identity])
            assert restored['records'] == before['records']
            assert restored['summary']['manifest'] == before['summary']['manifest']
            assert self.call('replay-'+label,['replay','--id',identity])['run']['execution'] == identity


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('binary','library','rom','production','linker','nm','output'):
        parser.add_argument('--'+name,type=pathlib.Path,required=True)
    parser.add_argument('--limit-mode',choices=['kernel','watchdog'],required=True)
    g = Gain(parser.parse_args())
    g.coefficient_boundaries()
    g.characterize()
    g.wifi()
    g.publish_wifi()
    g.bluetooth()
    g.additive()
    g.negative()
    g.preserve()
    print('authenticated gain arithmetic/publication and source-free replay passed',g.run,flush=True)


if __name__ == '__main__':
    main()
