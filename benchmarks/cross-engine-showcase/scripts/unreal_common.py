import unreal as u
HIGH_CVARS={'r.ScreenPercentage':66.666667,'r.AntiAliasingMethod':4.,'r.RayTracing':0.,'r.Lumen.HardwareRayTracing':0.,'r.DynamicRes.OperationMode':0.,'r.EyeAdaptation.LensAttenuation':.65,'r.MotionBlurQuality':0.,'sg.ViewDistanceQuality':2.,'sg.AntiAliasingQuality':2.,'sg.ShadowQuality':2.,'sg.GlobalIlluminationQuality':2.,'sg.ReflectionQuality':2.,'sg.PostProcessQuality':2.,'sg.TextureQuality':2.,'sg.EffectsQuality':2.,'sg.FoliageQuality':2.,'sg.ShadingQuality':2.}
def high_overrides(config):
    console=config.find_or_add_setting_by_class(u.MoviePipelineConsoleVariableSetting)
    for name,value in HIGH_CVARS.items():assert console.add_or_update_console_variable(name,value),name
