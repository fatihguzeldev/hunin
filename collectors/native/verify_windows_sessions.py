#!/usr/bin/env python3
"""Windows ETW controls: occupied logger refusal and owned cancellation cleanup."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

HERE=Path(__file__).resolve().parent

def execute(command,cwd,env=None,codes=(0,)):
    result=subprocess.run([str(v) for v in command],cwd=cwd,env=env,capture_output=True,text=True,timeout=90)
    if result.returncode not in codes:
        raise RuntimeError(f"Control failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}")
    return result.stdout.strip()

def verify(binary,output,sampler=None):
    if os.name!='nt':
        raise RuntimeError('This is an actual Windows ETW control; run it on Windows')
    if output.exists() and any(output.iterdir()):
        raise RuntimeError(f'Preserving existing evidence: {output}')
    output.mkdir(parents=True,exist_ok=True)
    environment=dict(os.environ)
    if sampler: environment['HUNIN_SAMPLY']=str(sampler)
    handle=None
    control=None
    recording=None
    direct=None
    release=None
    foreign=output/'external-control.etl'
    try:
        source=output/'source';source.mkdir()
        shutil.copy2(HERE/'fixtures'/'etw_ownership.c',source)
        (source/'CMakeLists.txt').write_text('cmake_minimum_required(VERSION 3.16)\nproject(etw_ownership C)\nadd_executable(etw_ownership etw_ownership.c)\ntarget_link_libraries(etw_ownership advapi32)\n')
        build=output/'build';cmake=os.environ.get('HUNIN_TEST_CMAKE','cmake')
        execute([cmake,'-S',source,'-B',build],output)
        execute([cmake,'--build',build,'--config','RelWithDebInfo'],output)
        candidates=list(build.rglob('etw_ownership.exe'))
        assert len(candidates)==1,candidates
        control=candidates[0]
        assert execute([control,'query'],output)=='absent','Pre-existing kernel session must be stopped by its actual owner before running this control'
        handle=execute([control,'start',foreign],output)
        assert int(handle,16)>0,handle
        data=output/'recordings'
        marker=output/'should-not-start'
        refused=json.loads(execute([binary,'--data-dir',data,'run','--runtime','native','--cwd',source,'--timeout','10','--',control,'work',marker],output,environment,codes=(0,1)))
        (output/'refused.json').write_text(json.dumps(refused,indent=2))
        assert not marker.exists(),'Target was launched despite occupied kernel logger'
        assert refused['run']['status']=='failed',refused
        assert execute([control,'query'],output)==handle,'Hunin stopped or replaced an external control session'
        reports=list((data/'captures'/refused['run']['id']).glob('*.xperf-session.json'))
        assert len(reports)==1,reports
        report=json.loads(reports[0].read_text())
        assert report['state']=='refused' and report['sessions']==[],report
        execute([control,'stop',handle,foreign],output);handle=None
        assert execute([control,'query'],output)=='absent'
        marker=output/'owned-target-ready'
        command=[str(v) for v in [binary,'--data-dir',data,'run','--runtime','native','--cwd',source,'--timeout','30','--',control,'work',marker]]
        recording=subprocess.Popen(command,cwd=output,env=environment,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        deadline=time.monotonic()+20
        while not marker.exists():
            if recording.poll() is not None:
                out,err=recording.communicate();raise RuntimeError(f'Native control exited before target readiness: {out}\n{err}')
            if time.monotonic()>deadline: raise RuntimeError('Target readiness deadline expired')
            time.sleep(.05)
        runs=json.loads(execute([binary,'--data-dir',data,'call','runs.list','{}'],output,environment))['runs']
        active=[r for r in runs if r['status']=='running']
        assert len(active)==1,active
        run_id=active[0]['id']
        owned_handle=execute([control,'query'],output)
        assert owned_handle!='absent','No actual owned ETW session was started'
        execute([binary,'--data-dir',data,'call','runs.cancel',json.dumps({'run_id':run_id})],output,environment)
        stdout,stderr=recording.communicate(timeout=30)
        (output/'cancel.stderr.txt').write_text(stderr)
        assert recording.returncode in (0,1),(recording.returncode,stdout,stderr)
        canceled=json.loads(stdout);(output/'canceled.json').write_text(json.dumps(canceled,indent=2))
        run=canceled['run']
        assert run['status']=='canceled',run
        assert any('forced' in text for text in run['diagnostics']),run
        assert run['source']['native_session_cleanup']['acknowledged'] is True,run
        assert execute([control,'query'],output)=='absent','Owned ETW session survived completed cancellation'
        report=json.loads(next((data/'captures'/run_id).glob('*.xperf-session.json')).read_text())
        assert report['state']=='stopped' and report['sessions'],report
        assert report['owner']['wrapper_pid']==run['source']['supervised_pid'],report
        assert any(event['state']=='stop_requested' and event['details'].get('reason')=='supervisor_stop_request' for event in report['events']),report
        # An unexpected wrapper exit inside the Engine Job must receive the
        # same pre-force cleanup handshake as requested cancellation.
        marker=output/'engine-wrapper-exit-ready'
        command=[str(v) for v in [binary,'--data-dir',data,'run','--runtime','native','--cwd',source,'--timeout','30','--',control,'work',marker]]
        recording=subprocess.Popen(command,cwd=output,env=environment,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        deadline=time.monotonic()+20
        while not marker.exists():
            if recording.poll() is not None:
                out,err=recording.communicate();raise RuntimeError(f'Wrapper-exit control failed before readiness: {out}\n{err}')
            if time.monotonic()>deadline:raise RuntimeError('Engine wrapper-exit readiness deadline expired')
            time.sleep(.02)
        active=[r for r in json.loads(execute([binary,'--data-dir',data,'call','runs.list','{}'],output,environment))['runs'] if r['status']=='running']
        assert len(active)==1,active
        interrupted_id=active[0]['id']
        active_report=json.loads(next((data/'captures'/interrupted_id).glob('*.xperf-session.json')).read_text())
        wrapper=active_report['owner']
        assert wrapper['wrapper_pid']==active[0]['source']['supervised_pid'],active_report
        execute([control,'kill-wrapper',wrapper['wrapper_pid'],wrapper['wrapper_creation_100ns']],output)
        out,err=recording.communicate(timeout=30)
        (output/'engine-wrapper-exit.stderr.txt').write_text(err)
        assert recording.returncode==1,(recording.returncode,out,err)
        interrupted=json.loads(out);(output/'engine-wrapper-exit.json').write_text(json.dumps(interrupted,indent=2))
        assert interrupted['run']['status']=='failed',interrupted
        assert interrupted['run']['source']['collector_outcome']['exit_code']==42,interrupted
        assert interrupted['run']['source']['native_session_cleanup']['acknowledged'] is True,interrupted
        assert execute([control,'query'],output)=='absent','Engine wrapper exit left an owned ETW session active'
        # Killing only the actual Samply IPC client must also wake its retained
        # helper watch even if a file-channel lock never supplies an EOF.
        sampler_path=sampler or Path(environment.get('HUNIN_SAMPLY',''))
        if not sampler_path.is_file():
            sampler_path=binary.parent/'collectors/native/bin/samply.exe'
        if not sampler_path.is_file():raise RuntimeError('Patched sampler path required for direct wrapper-death control')
        death=output/'wrapper-death';death.mkdir()
        marker=death/'target-ready';release=death/'target-release'
        profile=death/'cpu.samply.json'
        direct=subprocess.Popen([str(v) for v in [sampler_path,'record','--save-only','--unstable-presymbolicate','--lifetime-output',death/'cpu.samply.lifetimes.json','--keep-etl','-o',profile,'--',control,'work',marker,release]],cwd=output,env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        deadline=time.monotonic()+15
        while not marker.exists():
            if direct.poll() is not None:raise RuntimeError(f'Direct sampler exited before readiness: {direct.returncode}')
            if time.monotonic()>deadline:raise RuntimeError('Direct target readiness deadline expired')
            time.sleep(.02)
        direct_pid=direct.pid
        direct.kill();direct.wait(timeout=5)
        death_report=death/'cpu.samply.xperf-session.json'
        deadline=time.monotonic()+8
        final_report=None
        while time.monotonic()<deadline:
            try:
                observed=json.loads(death_report.read_text())
                if observed['state']=='stopped':final_report=observed;break
            except (FileNotFoundError,json.JSONDecodeError):pass
            time.sleep(.02)
        assert final_report is not None,'Direct Samply wrapper death left unconfirmed ETW cleanup'
        assert final_report['owner']['wrapper_pid']==direct_pid,final_report
        assert any(event['state']=='stop_requested' and event['details'].get('reason')=='samply_client_exited' for event in final_report['events']),final_report
        assert execute([control,'query'],output)=='absent','Direct wrapper death left an owned ETW session active'
        release.write_text('stop owned direct fixture')
        evidence={'refused_run':refused['run']['id'],'external_handle_preserved':True,'canceled_run':run_id,'owned_handle':owned_handle,'cleanup_acknowledged':True,'kernel_session_absent_after_cancel':True,'engine_wrapper_exit_run':interrupted_id,'engine_wrapper_exit_cleanup':True,'direct_wrapper_pid':direct_pid,'direct_wrapper_death_cleanup':True}
        (output/'evidence.json').write_text(json.dumps(evidence,indent=2));return evidence
    except BaseException as error:
        (output/'failure.json').write_text(json.dumps({'error':str(error)},indent=2));raise
    finally:
        if release is not None:release.write_text('stop owned direct fixture')
        if direct is not None and direct.poll() is None:
            direct.kill();direct.wait(timeout=5)
        if recording is not None and recording.poll() is None:
            # Request through the owner. Never use a global xperf stop or kill
            # an unverified logger merely to make a failed control pass.
            try:
                runs=json.loads(execute([binary,'--data-dir',output/'recordings','call','runs.list','{}'],output,environment))['runs']
                for run in runs:
                    if run['status']=='running': execute([binary,'--data-dir',output/'recordings','call','runs.cancel',json.dumps({'run_id':run['id']})],output,environment)
                recording.communicate(timeout=30)
            except Exception: recording.kill();recording.wait(timeout=5)
        if handle is not None and control is not None:
            execute([control,'stop',handle,foreign],output)

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True);parser.add_argument('--output',type=Path,required=True);parser.add_argument('--sampler',type=Path)
    args=parser.parse_args()
    print(json.dumps(verify(args.binary.resolve(),args.output.resolve(),args.sampler.resolve() if args.sampler else None),indent=2))
