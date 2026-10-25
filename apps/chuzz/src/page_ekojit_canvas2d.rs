use std::sync::Arc;

use blitz_script_ekojit::{EkojitDocument, PageSource};

pub(super) const SOURCE: &str = r#"(function (g) {
  'use strict';
  var bridge = g.__chuzzCanvas2DBridge;
  if (!bridge || typeof bridge.request !== 'function') { throw new TypeError('native canvas bridge is unavailable'); }
  var contexts = new Map(), resets = new Map(), MAX_SIDE = 65535, MAX_BYTES = 64 * 1024 * 1024;
  function integer(value) { return Number.isSafeInteger(value) && value >= 0 && value <= MAX_SIDE; }
  function dimensions(canvas) {
    var w = Number(canvas.width), h = Number(canvas.height);
    if (!integer(w) || !integer(h) || w * h * 4 > MAX_BYTES) { throw new RangeError('canvas dimensions exceed native limits'); }
    return [w, h];
  }
  function handle(canvas) {
    var id = Number(canvas.__chuzzCanvasHandle);
    if (!Number.isSafeInteger(id) || id <= 0) { throw new TypeError('canvas has no native handle'); }
    return id;
  }
  function defaultState() {
    return { matrix:[1,0,0,1,0,0], alpha:1, composite:'source-over', width:1, cap:'butt', join:'miter', miter:10, dash:[], dashOffset:0,
      font:'10px sans-serif', kerning:'auto', fontStretch:'normal', fontVariantCaps:'normal', letterSpacing:0, wordSpacing:0, textRendering:'auto',
      align:'start', baseline:'alphabetic', direction:'inherit', smoothing:true, shadowBlur:0, shadowColor:'rgba(0, 0, 0, 0)', shadowOffsetX:0, shadowOffsetY:0, filter:[] };
  }
  function initial(canvas) { return { fill:'black', stroke:'black', state:defaultState(), stack:[], path:[] }; }
  function api(canvas) {
    var id = handle(canvas), dims = dimensions(canvas), c = initial(canvas), pending = [];
    function state() { var s = c.state, copy = {}; Object.keys(s).forEach(function (k) { copy[k] = Array.isArray(s[k]) ? s[k].slice() : s[k]; }); return copy; }
    function flush() {
      if (!pending.length) { return; }
      var commands = pending; pending = [];
      bridge.request({action:'submit',id:id,width:dims[0],height:dims[1],commands:commands}, []);
      bridge.request({action:'paint',id:id,width:dims[0],height:dims[1]}, []);
    }
    function submit(op, n, brush, text) {
      if (pending.length >= 128) { flush(); }
      pending.push({op:op,n:n || [],matrix:[1,0,0,1,0,0],state:state(),brush:brush || {kind:'solid',color:c.fill},text:text || ''});
      flush();
    }
    function reset() {
      dims = dimensions(canvas); pending = []; c = initial(canvas);
      bridge.request({action:'reset',id:id,width:dims[0],height:dims[1]}, []);
      bridge.request({action:'paint',id:id,width:dims[0],height:dims[1]}, []);
    }
    function validStyle(v) {
      if (typeof v !== 'string' || !v.trim() || v.length > 4096) { return false; }
      if (g.CSS && typeof g.CSS.supports === 'function') { return g.CSS.supports('color', v); }
      return /^(#[0-9a-f]{3,8}|rgba?\([^)]*\)|hsla?\([^)]*\)|[a-z]+)$/i.test(v.trim());
    }
    function install(name, get, set) { Object.defineProperty(ctx, name, {enumerable:true, configurable:false, get:get, set:set}); }
    var ctx = {canvas:canvas};
    install('fillStyle', function(){return c.fill;}, function(v){if(validStyle(v)) c.fill=String(v);});
    install('strokeStyle', function(){return c.stroke;}, function(v){if(validStyle(v)) c.stroke=String(v);});
    install('globalAlpha', function(){return c.state.alpha;}, function(v){v=Number(v);if(Number.isFinite(v)&&v>=0&&v<=1)c.state.alpha=v;});
    install('globalCompositeOperation', function(){return c.state.composite;}, function(v){var list=['source-over','source-in','source-out','source-atop','destination-over','destination-in','destination-out','destination-atop','lighter','copy','xor','multiply','screen','overlay','darken','lighten','color-dodge','color-burn','hard-light','soft-light','difference','exclusion','hue','saturation','color','luminosity'];if(list.indexOf(String(v))>=0)c.state.composite=String(v);});
    install('font', function(){return c.state.font;}, function(v){if(typeof v==='string'&&v.length<=1024&&/\d+(\.\d+)?(px|pt|em|rem)\s+.+/.test(v))c.state.font=v;});
    install('textAlign', function(){return c.state.align;}, function(v){if(['start','end','left','right','center'].indexOf(v)>=0)c.state.align=v;});
    install('textBaseline', function(){return c.state.baseline;}, function(v){if(['top','hanging','middle','alphabetic','ideographic','bottom'].indexOf(v)>=0)c.state.baseline=v;});
    install('direction', function(){return c.state.direction;}, function(v){if(['ltr','rtl','inherit'].indexOf(v)>=0)c.state.direction=v;});
    install('lineWidth', function(){return c.state.width;}, function(v){v=Number(v);if(Number.isFinite(v)&&v>0&&v<=1e7)c.state.width=v;});
    install('lineCap', function(){return c.state.cap;}, function(v){if(['butt','round','square'].indexOf(v)>=0)c.state.cap=v;});
    install('lineJoin', function(){return c.state.join;}, function(v){if(['round','bevel','miter'].indexOf(v)>=0)c.state.join=v;});
    install('miterLimit', function(){return c.state.miter;}, function(v){v=Number(v);if(Number.isFinite(v)&&v>0&&v<=1e7)c.state.miter=v;});
    install('imageSmoothingEnabled', function(){return c.state.smoothing;}, function(v){c.state.smoothing=!!v;});
    install('lineDashOffset', function(){return c.state.dashOffset;}, function(v){v=Number(v);if(Number.isFinite(v))c.state.dashOffset=v;});
    ctx.save=function(){if(c.stack.length<256)c.stack.push({fill:c.fill,stroke:c.stroke,state:state()});};
    ctx.restore=function(){var x=c.stack.pop();if(x){c.fill=x.fill;c.stroke=x.stroke;c.state=x.state;}};
    ctx.setTransform=function(a,b,d,e,tx,ty){var m;if(arguments.length===1&&a&&typeof a==='object')m=[a.a,a.b,a.c,a.d,a.e,a.f];else m=[a,b,d,e,tx,ty].map(Number);if(m.length===6&&m.every(Number.isFinite))c.state.matrix=m;};
    ctx.resetTransform=function(){c.state.matrix=[1,0,0,1,0,0];};
    ctx.transform=function(a,b,d,e,tx,ty){var m=[a,b,d,e,tx,ty].map(Number),o=c.state.matrix;if(!m.every(Number.isFinite))return;c.state.matrix=[o[0]*m[0]+o[2]*m[1],o[1]*m[0]+o[3]*m[1],o[0]*m[2]+o[2]*m[3],o[1]*m[2]+o[3]*m[3],o[0]*m[4]+o[2]*m[5]+o[4],o[1]*m[4]+o[3]*m[5]+o[5]];};
    ctx.translate=function(x,y){ctx.transform(1,0,0,1,Number(x),Number(y));};
    ctx.scale=function(x,y){ctx.transform(Number(x),0,0,Number(y),0,0);};
    ctx.rotate=function(a){a=Number(a);if(Number.isFinite(a)){var co=Math.cos(a),si=Math.sin(a);ctx.transform(co,si,-si,co,0,0);}};
    ctx.fillRect=function(x,y,w,h){submit('fillRect',[Number(x),Number(y),Number(w),Number(h)],{kind:'solid',color:c.fill});};
    ctx.strokeRect=function(x,y,w,h){submit('strokeRect',[Number(x),Number(y),Number(w),Number(h)],{kind:'solid',color:c.stroke});};
    ctx.clearRect=function(x,y,w,h){submit('clearRect',[Number(x),Number(y),Number(w),Number(h)],{kind:'solid',color:'rgba(0,0,0,0)'});};
    ctx.fillText=function(text,x,y,maxWidth){var n=[Number(x),Number(y),arguments.length>3?Number(maxWidth):0];submit('fillText',n,{kind:'solid',color:c.fill},String(text));};
    ctx.strokeText=function(text,x,y,maxWidth){var n=[Number(x),Number(y),arguments.length>3?Number(maxWidth):0];submit('strokeText',n,{kind:'solid',color:c.stroke},String(text));};
    ctx.measureText=function(text){flush();return bridge.request({action:'measure',id:id,width:dims[0],height:dims[1],text:String(text),state:state()},[]);};
    ctx.setLineDash=function(a){if(!Array.isArray(a)||a.length>1024)return;var d=a.map(Number);if(d.some(function(x){return !Number.isFinite(x)||x<0||x>1e7;}))return;if(d.length%2)d=d.concat(d);c.state.dash=d;};
    ctx.getLineDash=function(){return c.state.dash.slice();};
    function imageData(width,height,data) {
      width=Number(width);height=Number(height);
      if(!Number.isSafeInteger(width)||!Number.isSafeInteger(height)||width<=0||height<=0||width*height*4>MAX_BYTES)throw new RangeError('invalid ImageData dimensions');
      if(data===undefined)data=new Uint8ClampedArray(width*height*4);
      if(!(data instanceof Uint8ClampedArray)||data.length!==width*height*4)throw new TypeError('ImageData requires a matching Uint8ClampedArray');
      return {width:width,height:height,data:data,colorSpace:'srgb'};
    }
    ctx.createImageData=function(a,b){if(a&&typeof a==='object')return imageData(a.width,a.height);return imageData(a,b);};
    ctx.getImageData=function(x,y,w,h){flush();x=Number(x);y=Number(y);w=Number(w);h=Number(h);if(![x,y,w,h].every(Number.isFinite)||w===0||h===0)throw new TypeError('invalid getImageData rectangle');if(w<0){x+=w;w=-w;}if(h<0){y+=h;h=-h;}x=Math.trunc(x);y=Math.trunc(y);w=Math.trunc(w);h=Math.trunc(h);if(w<=0||h<=0||w*h*4>MAX_BYTES)throw new RangeError('invalid getImageData dimensions');var bytes=bridge.copyBytes(bridge.request({action:'read',id:id,width:dims[0],height:dims[1],n:[x,y,w,h]},[]));return imageData(w,h,new Uint8ClampedArray(bytes));};
    ctx.putImageData=function(im,dx,dy,dirtyX,dirtyY,dirtyW,dirtyH){
      if(!im||!Number.isSafeInteger(Number(im.width))||!Number.isSafeInteger(Number(im.height))||Number(im.width)<=0||Number(im.height)<=0||!(im.data instanceof Uint8ClampedArray)||im.data.length!==Number(im.width)*Number(im.height)*4)throw new TypeError('expected valid ImageData');
      var sx=0,sy=0,sw=Number(im.width),sh=Number(im.height);if(arguments.length>=7){sx=Number(dirtyX);sy=Number(dirtyY);sw=Number(dirtyW);sh=Number(dirtyH);if(![sx,sy,sw,sh].every(Number.isFinite))throw new TypeError('invalid dirty rectangle');var ex=sx+sw,ey=sy+sh;sx=Math.max(0,Math.ceil(Math.min(sx,ex)));sy=Math.max(0,Math.ceil(Math.min(sy,ey)));sw=Math.max(0,Math.floor(Math.max(Number(dirtyX),ex))-sx);sh=Math.max(0,Math.floor(Math.max(Number(dirtyY),ey))-sy);}
      flush();var bytes=bridge.copyBytes(im.data);bridge.request({action:'submit',id:id,width:dims[0],height:dims[1],commands:[{op:'putImageData',n:[Number(dx),Number(dy),sx,sy,sw,sh],matrix:[1,0,0,1,0,0],brush:{kind:'solid',color:'black',matrix:[1,0,0,1,0,0],image:{index:0,width:Number(im.width),height:Number(im.height),offset:0,length:bytes.length}}}]},[bytes]);bridge.request({action:'paint',id:id,width:dims[0],height:dims[1]},[]);
    };
    ctx.toDataURL=function(type){flush();if(!dims[0]||!dims[1])return 'data:,';var bytes=bridge.copyBytes(bridge.request({action:'snapshot',id:id,width:dims[0],height:dims[1]},[]));var png=bridge.copyBytes(g.__chuzzEncodePng(dims[0],dims[1],bytes));var alphabet='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/',encoded='';for(var i=0;i<png.length;i+=3){var a=png[i],b=i+1<png.length?png[i+1]:0,c=i+2<png.length?png[i+2]:0;encoded+=alphabet[a>>2]+alphabet[((a&3)<<4)|(b>>4)]+(i+1<png.length?alphabet[((b&15)<<2)|(c>>6)]:'=')+(i+2<png.length?alphabet[c&63]:'=');}return 'data:image/png;base64,'+encoded;};
    ctx.drawImage=function(source){flush();return bridge.drawImage(ctx,source,arguments);};
    resets.set(id, reset);
    return ctx;
  }
  function create(canvas) { var id=handle(canvas), old=contexts.get(id);if(old&&old.canvas===canvas)return old;var c=api(canvas);contexts.set(id,c);canvas.__chuzz2dContext=c;bridge.request({action:'reset',id:id,width:Number(canvas.width),height:Number(canvas.height)},[]);return c; }
  g.__chuzzCreateCanvasContext=function(canvas,kind,options){kind=String(kind).toLowerCase();if(kind==='2d'){if(canvas.__chuzzContextKind&&canvas.__chuzzContextKind!=='2d')return null;canvas.__chuzzContextKind='2d';return canvas.__chuzz2dContext||create(canvas);}if(typeof bridge.createOther==='function')return bridge.createOther(canvas,kind,options);return null;};
  g.__chuzzCanvasResize=function(canvas){var id=Number(canvas.__chuzzCanvasHandle),c=contexts.get(id),w=Number(canvas.width),h=Number(canvas.height);if(!Number.isSafeInteger(id)||id<=0||!Number.isSafeInteger(w)||!Number.isSafeInteger(h)||w<0||h<0||w>MAX_SIDE||h>MAX_SIDE||w*h*4>MAX_BYTES)throw new RangeError('canvas dimensions exceed native limits');if(c){resets.get(id)();}};
  g.__chuzzCanvasDataURL=function(canvas,type){if(canvas.__chuzzContextKind&&canvas.__chuzzContextKind!=='2d')return 'data:,';return (canvas.__chuzz2dContext||create(canvas)).toDataURL(type);};
})(globalThis);"#;

pub(super) fn install(document: &mut EkojitDocument) -> Result<(), String> {
    document
        .eval_source(Arc::new(PageSource {
            text: Arc::from(SOURCE),
            url: Arc::from("chuzz:page-bootstrap/canvas2d"),
        }))
        .map(|_| ())
        .map_err(|error| format!("could not install native Canvas 2D: {error}"))
}
