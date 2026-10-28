set -eu
case_dir=/home/islam/ara-ux-editor/target/ux-p4
actual_exe=$(readlink /proc/2435493/exe)
actual_parent=$(ps -p 2435493 -o ppid= | tr -d ' ')
parent_exe=$(readlink /proc/2435483/exe)
[ "$actual_exe" = /home/islam/ara-ux-editor/target/debug/deps/ara_editor-cdd3436b7259cfc8 ]
[ "$actual_parent" = 2435483 ]
[ "$parent_exe" = /usr/bin/gdb ]
ps -p 2435493,2435483 -o pid,ppid,lstart,comm >"$case_dir/resource-gdb-interruption.txt"
printf '%s\n' 'Interrupting only this owned GDB inferior after both tests passed but process teardown hung, to obtain thread stacks.' >>"$case_dir/resource-gdb-interruption.txt"
kill -INT 2435493