"""Close the owned editor on setup errors, even after a script enables keep-alive."""
import unreal as u,os,traceback,sys
from pathlib import Path
script=Path(os.environ['ARABENCH_UNREAL_SCRIPT'])
sys.path.insert(0,str(script.parent))
try:exec(compile(script.read_text(),str(script),'exec'),globals())
except Exception:
    error=traceback.format_exc();print('ARABENCH_SCRIPT_ERROR\n'+error)
    Path(r'D:\AraBench\logs\unreal-script-error.txt').write_text(error)
    u.EditorPythonScripting.set_keep_python_script_alive(False);u.SystemLibrary.quit_editor()
