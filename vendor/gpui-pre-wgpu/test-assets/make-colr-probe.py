from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.colorLib.builder import buildCOLR, buildCPAL
from fontTools.ttLib.tables.otTables import PaintFormat, CompositeMode
fb=FontBuilder(1000,isTTF=True)
names=[".notdef","shape","gradient","out","in","foreground"]
fb.setupGlyphOrder(names)
glyphs={}
for name in names:
 p=TTGlyphPen(None)
 if name=="shape":
  p.moveTo((0,500));p.qCurveTo((0,1000),(500,1000));p.qCurveTo((1000,1000),(1000,500));p.qCurveTo((1000,0),(500,0));p.qCurveTo((0,0),(0,500));p.closePath()
 glyphs[name]=p.glyph()
fb.setupGlyf(glyphs)
fb.setupHorizontalMetrics({n:(1100,0) for n in names})
fb.setupHorizontalHeader(ascent=1000,descent=0)
fb.setupCharacterMap({0x6D:"shape",0xE000:"foreground",0xE001:"gradient",0xE002:"out",0xE003:"in",0x416:"foreground",0x424:"gradient",0x426:"out",0x428:"in"})
fb.setupNameTable({"familyName":"COLR Probe","styleName":"Regular","uniqueFontIdentifier":"COLRProbe","fullName":"COLR Probe","psName":"COLRProbe"})
fb.setupOS2(sTypoAscender=1000,sTypoDescender=0,usWinAscent=1000,usWinDescent=0)
fb.setupPost();fb.setupMaxp()
def solid(i):return {"Format":PaintFormat.PaintSolid,"PaletteIndex":i,"Alpha":1.0}
def shape(p):return {"Format":PaintFormat.PaintGlyph,"Glyph":"shape","Paint":p}
colors={"foreground":shape(solid(0xffff)),
"gradient":shape({"Format":PaintFormat.PaintLinearGradient,"ColorLine":{"Extend":0,"ColorStop":[{"StopOffset":0.0,"PaletteIndex":0,"Alpha":1.0},{"StopOffset":1.0,"PaletteIndex":1,"Alpha":1.0}]},"x0":0,"y0":0,"x1":1000,"y1":0,"x2":0,"y2":1000})}
for name,mode in [("out",CompositeMode.SRC_OUT),("in",CompositeMode.SRC_IN)]:
 colors[name]=shape({"Format":PaintFormat.PaintComposite,"SourcePaint":solid(0),"CompositeMode":mode,"BackdropPaint":solid(1)})
fb.font["COLR"]=buildCOLR(colors,version=1,glyphMap=fb.font.getReverseGlyphMap())
fb.font["CPAL"]=buildCPAL([[(1,0,0,1),(0,0,1,1)]])
from pathlib import Path
fb.save(Path(__file__).with_name("COLRProbe.ttf"))

# Valid COLRv1 paint exceeds the renderer's stop budget, but the base glyph
# still has a drawable monochrome outline.
fb.font["glyf"]["foreground"] = glyphs["shape"]
colors["foreground"] = shape({
 "Format":PaintFormat.PaintLinearGradient,
 "ColorLine":{"Extend":0,"ColorStop":[
  {"StopOffset":i/64,"PaletteIndex":0,"Alpha":1.0} for i in range(65)
 ]},
 "x0":0,"y0":0,"x1":1000,"y1":0,"x2":0,"y2":1000,
})
fb.font["COLR"] = buildCOLR(colors,version=1,glyphMap=fb.font.getReverseGlyphMap())
fb.save(Path(__file__).with_name("COLRRejected.ttf"))
