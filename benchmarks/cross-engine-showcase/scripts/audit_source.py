"""Check render visibility, including importer helper collections, without rendering."""
import bpy,json,sys
from pathlib import Path
root=Path(r'D:\AraBench');name=sys.argv[sys.argv.index('--')+1];folder=root/'source'/name
bpy.ops.wm.open_mainfile(filepath=str(folder/(name+'.blend')))
visible=set()
def visit(collection,parent_visible):
    current=parent_visible and not collection.hide_render
    if current:visible.add(collection.name)
    for child in collection.children:visit(child,current)
visit(bpy.context.scene.collection,True)
meshes=[o for o in bpy.context.scene.objects if o.type=='MESH'];rendered=[o for o in meshes if not o.hide_render and any(c.name in visible for c in o.users_collection)]
triangles=sum(len(p.vertices)-2 for o in rendered for p in o.data.polygons)
audit={'mesh_objects':len(meshes),'rendered_mesh_objects':len(rendered),'rendered_triangles':triangles,'excluded':[{'name':o.name,'polygons':len(o.data.polygons),'collections':[(c.name,c.hide_render)for c in o.users_collection]}for o in meshes if o not in rendered]}
(root/'report/images'/name/'cycles/source-visibility.json').write_text(json.dumps(audit,indent=2));m=json.loads((folder/'manifest.json').read_text());m['renderedTriangles']=triangles;m['renderedMeshObjects']=len(rendered);(folder/'manifest.json').write_text(json.dumps(m,indent=2));print(json.dumps(audit))
