import os
import subprocess

env = dict(os.environ, CARGO_BUILD_JOBS='1', RUST_TEST_THREADS='1')
commands = [
    ['cargo','fmt','--all','--check'],
    ['cargo','test','--locked','--offline','-p','pagura-render','--lib','word_output_matches_rational_rounding_for_every_word','--','--test-threads=1'],
    ['cargo','test','--locked','--offline','-p','pagura-render','--lib','direct_root_backgrounds_remain_premultiplied_in_both_modes','--','--test-threads=1'],
    ['cargo','clippy','--locked','--offline','-p','pagura-render','--all-targets','--','-D','warnings'],
    ['cargo','build','--locked','--offline','--release','-p','pagura-render','--example','scene_index_profile','-j','1'],
    ['cargo','build','--locked','--offline','-p','pagura-host','-j','1'],
    ['cargo','build','--locked','--offline','--release','-p','pagura-host','--target','wasm32-wasip1','-j','1'],
]
for command in commands:
    print('Checking:', ' '.join(command), flush=True)
    subprocess.run(command,check=True,env=env)
