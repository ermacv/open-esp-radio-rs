"""Captured calibration storage and RF-test power-policy characterization.

No production storage or RF power API is inferred from these vendor observations.
All state changes execute captured instructions through native warm phases.
Storage needs only the pinned PHY archive and ROM. Without the authenticated
RF-test archive the producer is reported as an unmet obligation, never omitted.
"""
import copy

from harness import case, region, selection, words
from phy_gain import INPUT, OUTPUT, arithmetic, calls, events, gain_models, output, publication, signed8

CACHE, INIT = 0x3fff5000, 0x3fff6000
# Independently read policy examples, including truncating remainder and i8 wrap.
POWER = [(80,0,0,20,False),(80,1,3,19,False),(80,2,2,19,False),(80,3,1,19,False),
         (80,4,0,19,False),(80,-1,1,20,False),(80,-2,2,20,False),(80,-3,3,20,False),
         (80,-4,0,21,False),(80,-5,1,21,True),(80,-8,4,21,True),(84,-1,1,21,True),
         (80,-48,0,-32,False),(80,127,1,-12,False),(80,-128,0,-12,False)]


RFTEST_OBLIGATION = dict(id='rftest-power-producer', unit='12.5',
    reason='authenticated librftest.a was not supplied; producer, rounding/saturation and gain/MAC publication cases did not execute')


def mac_power(index, initial):
    """Two read-modify-write updates of the 6-bit index fields at bits 0 and 8."""
    first = (initial&~63)|(index&63)
    second = (first&~(63<<8))|((index&63)<<8)
    return [('read',0x20105500,initial),('write',0x20105500,first),
            ('read',0x20105500,first),('write',0x20105500,second)]


def storage(g):
    request = dict(occurrence=dict(revision=g.revision,source=g.vendor['source'],object=g.image_object,symbol=None),
                   analyses=[],ranges=[dict(kind='image',address=g.parameter+434,length=1)])
    g.call('startup-adjustment',['data','--request',g.doc('startup-adjustment',request),'--output',g.run/'startup-adjustment'])
    assert (g.run/'startup-adjustment/data.bin').read_bytes() == b'\0'
    for fill in (0x5a,0xa5):
        adjustments = [0,1,31,127,128,255]
        for batch in range(0,6,2):
            rows,states = [],[]
            for adjustment in adjustments[batch:batch+2]:
                state = [(i*37+fill)&255 for i in range(516)]
                state[434] = adjustment
                states.append(state)
                setup = g.setup(state)
                setup['observe_memory'] = [selection(g.parameter,516)]
                rows.append(case('seed-complete-state',setup,None))
                phases = [
                    ('backup',g.enter(g.roots['phy_rf_cal_data_backup_new'],[CACHE],
                        [region(CACHE,532,fill=fill,lifetime='session')],[selection(CACHE,532)])),
                    ('destroy-live-state',g.setup([v^255 for v in state])),
                    ('recover-saved-state',g.enter(g.roots['phy_rf_cal_data_recovery_new'],[CACHE])),
                    ('register-init-parameters',g.enter(g.roots['register_chipv7_phy_init_param'],[INIT],
                        [region(INIT,128,fill=fill)])),
                    ('isolate-curve',g.enter(g.sym(1,'memset')['value'],[g.parameter+241,0,7])),
                    ('isolate-base',g.enter(g.sym(1,'memset')['value'],[g.parameter+291,0,1])),
                    ('consume-restored-adjustment',g.wifi_phase(13,fill)),
                    ('clear-adjustment',g.enter(g.sym(1,'memset')['value'],[g.parameter+434,0,1])),
                    ('move-adjustment-to-base',g.enter(g.sym(1,'memcpy')['value'],[g.parameter+291,INPUT,1],
                        [region(INPUT,1,[adjustment])])),
                    ('consume-normalized-base',g.wifi_phase(13,fill)),
                ]
                for name,phase in phases:
                    if not phase['observe_memory']:
                        phase['observe_memory'] = [selection(g.parameter,516)]
                    rows.append(case(name,phase,None,reset='warm'))
            records,request,identity = g.execute(f'storage-{fill}-{batch}',rows,fill,False)
            assert not any(r['kind']=='event' for r in records)
            for i,state in enumerate(states):
                offset = 11*i
                assert output(records,offset)==bytes(state)
                assert output(records,offset+1)==bytes([fill]*12+state+[fill]*4)
                assert output(records,offset+2)==bytes(v^255 for v in state)
                assert output(records,offset+3)==bytes(state)
                # Exact byte writes of the captured parameter registration loops.
                initialized = state.copy()
                initialized[78] = fill
                initialized[80:98] = [fill]*18
                initialized[100:152] = [fill]*52
                assert output(records,offset+4)==bytes(initialized)
                initialized[241:248] = [0]*7
                assert output(records,offset+5)==bytes(initialized)
                initialized[291] = 0
                assert output(records,offset+6)==bytes(initialized)
                expected = arithmetic(g.coefficients[108:],[0]*6,signed8(state[434]),0,13)
                assert output(records,offset+7)==expected
                initialized[434] = 0
                assert output(records,offset+8)==bytes(initialized)
                initialized[291] = state[434]
                assert output(records,offset+9)==bytes(initialized)
                assert output(records,offset+10)==expected
            if batch==0 and fill==0x5a:
                baseline = request,identity

    return baseline


def producer(g):
    for fill in (0x5a,0xa5):
        for batch in range(0,len(POWER),3):
            rows = []
            for target,attenuation,adjustment,index,publish in POWER[batch:batch+3]:
                data = [0]*516
                data[80:98] = [target&255]*18
                data[6],data[8],data[284],data[434] = 84,attenuation&255,13,0x5a
                rows.append(case('seed-power-inputs',g.setup(data),None))
                install = g.enter(g.roots['phy_get_romfunc_addr'],memory=[
                    region(0x2f07fc3c,4,words([0x2f07f944]),lifetime='session'),region(0x2f07fc40,4,lifetime='session')],
                    observe=[selection(0x2f07f968,4)])
                rows.append(case('install-current-callbacks',install,None,reset='warm'))
                models = gain_models(0,0)
                models[0]['behavior']['value'] = 0
                models.append(dict(id='test-mac-power',applicability='explicit retained test MAC word; no power selection algorithm',
                    lifetime='phase',behavior=dict(kind='register-bank',cells=[dict(address=0x20105500,width=4,value=0xa5a5a5a5)])))
                phase = g.enter(g.roots['set_rate_power_index'],[0],models=models,observe=[selection(g.parameter+434,1)])
                phase['observe_calls'] = dict(include_tail=True,argument_words=0,overrides=[
                    dict(target=g.roots['phy_wifi_set_tx_gain_new'],words=2),dict(target=g.roots['mac_power_set'],words=1)])
                rows.append(case(f'power-{target}-{attenuation}',phase,None,reset='warm'))
            records,request,identity = g.execute(f'power-{fill}-{batch}',rows,fill,False)
            for i,(target,attenuation,adjustment,index,publish) in enumerate(POWER[batch:batch+3]):
                assert output(records,3*i+1)==g.roots['phy_wifi_get_tx_tab_new'].to_bytes(4,'little')
                assert not events(records,3*i) and not events(records,3*i+1)
                assert output(records,3*i+2)==bytes([adjustment])
                observed = events(records,3*i+2)
                assert calls(observed,g.roots['phy_wifi_set_tx_gain_new'])==([[13,0]] if publish else [])
                assert calls(observed,g.roots['mac_power_set'])==[[index&0xffffffff]]
                expected = []
                if publish:
                    calculated = arithmetic(g.coefficients[108:],[0]*6,adjustment,0,13)
                    expected = publication([0]*6,0,calculated,32,0,0)
                    expected[0] = ('read',0x20100408,0)
                expected += mac_power(index,0xa5a5a5a5)
                effects = [e for e in observed if e['kind'] not in ('call-transfer','transfer-argument')]
                assert all(e['width']==4 for e in effects)
                assert [(e['kind'],e['address'],e['value']) for e in effects]==expected, (target,attenuation,fill)
            if batch==9 and fill==0x5a:
                publishing = request

    return publishing


def storage_negative(g, baseline):
    storage_request, storage_identity = baseline
    rows = []
    for source in storage_request['cases'][:8]:
        phase = source['vendor']
        rows.append(case(source['name'],phase,copy.deepcopy(phase),reset=source['reset'],
                         memory=bool(phase['observe_memory'])))
    def mutate(value):
        return g.enter(g.sym(1,'memcpy')['value'],[CACHE+12+434,INPUT,1],
            [region(INPUT,1,[value])],[selection(CACHE+12+434,1)])
    # The baseline adjustment is zero; both sides overwrite it with distinct values.
    seeded = next(m for m in storage_request['cases'][0]['vendor']['memory'] if m['seed']['address']==INPUT)
    assert seeded['seed']['bytes'][434]==0
    rows.insert(2,case('corrupt-saved-adjustment',mutate(1),mutate(31),reset='warm',memory=True))
    records,_,_ = g.execute('storage-corruption-propagates',rows,0x5a,verdict='DIFF',right_target=g.vendor)
    assert output(records,2)==bytes([1]) and output(records,2,True)==bytes([31])
    assert output(records,4)[434]==1 and output(records,4,True)[434]==31
    assert output(records,8)==arithmetic(g.coefficients[108:],[0]*6,1,0,13)
    assert output(records,8,True)==arithmetic(g.coefficients[108:],[0]*6,31,0,13)
    assert not any(r['kind']=='event' for r in records)
    # This documents that the vendor copy itself does not validate corruption;
    # no production storage or checksum guarantee is introduced.
    recovery = g.enter(g.roots['phy_rf_cal_data_recovery_new'],[CACHE],
        [region(CACHE,532)],[selection(g.parameter,516)])
    rows = [case('seed-live-state',g.setup([0]*516),None),
            case('unknown-cache',recovery,None,reset='warm')]
    records,_,_ = g.execute('storage-unknown-cache',rows,0x5a,False,verdict='INCOMPLETE')
    assert not g.artifacts[-1][2]['summary']['manifest']['complete']
    stop = next(r['stop'] for r in records if r['kind']=='outcome' and r['case']==1)
    assert stop['kind']=='incomplete' and stop['reason']['kind']=='memory',stop
    assert not any(r['kind']=='event' for r in records)
    capacity(g, storage_request, storage_identity)


def power_negative(g, power_request):
    rows = copy.deepcopy(power_request['cases'][:3])
    del rows[1]
    # Supply only the parameter pointer. The absent callback installation must
    # stop gain regeneration even though the parent already wrote adjustment.
    rows[1]['vendor']['memory'].append(region(0x2f07fc40,4,words([g.parameter])))
    records,_,_ = g.execute('power-missing-callback',rows,0x5a,False,verdict='INCOMPLETE')
    assert not g.artifacts[-1][2]['summary']['manifest']['complete']
    stop = next(r['stop'] for r in records if r['kind']=='outcome' and r['case']==1)
    assert stop['kind']=='incomplete',stop
    assert output(records,1)==bytes([1])
    observed = events(records,1)
    assert calls(observed,g.roots['phy_wifi_set_tx_gain_new'])==[[13,0]]
    assert not calls(observed,g.roots['mac_power_set'])
    assert not [e for e in observed if e['kind'] not in ('call-transfer','transfer-argument')]

    # Leave the attenuation byte and later policy inputs unknown. Loading an
    # unknown byte stops the seed phase, so the policy phase cannot select a
    # gain or MAC index from partially known parameters.
    rows = copy.deepcopy(power_request['cases'][:3])
    source = next(m for m in rows[0]['vendor']['memory'] if m['seed']['address']==INPUT)
    source['seed']['bytes'],source['seed']['fill'] = source['seed']['bytes'][:8],None
    records,_,_ = g.execute('power-unknown-attenuation',rows,0x5a,False,verdict='INCOMPLETE')
    assert not g.artifacts[-1][2]['summary']['manifest']['complete']
    stops = {r['case']:r['stop'] for r in records if r['kind']=='outcome'}
    assert stops[0]['kind']=='incomplete' and stops[0]['reason']==dict(kind='memory',address=INPUT+8,access='read'),stops
    assert stops[1]==stops[2]==dict(kind='blocked-by-prior-phase'),stops
    assert not events(records,1) and not events(records,2)


def capacity(g, storage_request, storage_identity):
    before = g.call('storage-before-failure',['execution','--id',storage_identity])
    limited = copy.deepcopy(storage_request)
    limited['cases'] = limited['cases'][:2]
    limited['cases'][1]['vendor']['observe_calls'] = dict(include_tail=True,argument_words=3,overrides=[])
    limited['max_events'] = 1
    failed = g.call('storage-capacity',['execute','--request',g.doc('storage-capacity',limited)],expected=1)['run']
    assert failed['error']['code']=='resource-limited'
    assert failed.get('execution') is None and failed.get('publication') is None and failed.get('resolved_operation') is None
    after = g.call('storage-after-failure',['execution','--id',storage_identity])
    assert after['records']==before['records'] and after['summary']['manifest']==before['summary']['manifest']


def exercise(g, rftest):
    """Run every available 12.5 case; return obligations that could not execute."""
    storage_negative(g,storage(g))
    if not rftest:
        return [RFTEST_OBLIGATION]
    power_negative(g,producer(g))
    return []
