const __vite__mapDeps=(i,m=__vite__mapDeps,d=(m.f||(m.f=["./webgl-device-McYZ27Ud.js","./rolldown-runtime-DK3Fl9T5.js","./array-utils-flat-Bz_dufVy.js","./get-attribute-from-layouts-DnJQzRbL.js","./webgl-Rrlc2Lyq.js","./gpu-data-evaluator-DowrXdaM.js","./buffer-transform-ZALaEdOc.js","./webgpu-yD2IvmEH.js"])))=>i.map(i=>d[i]);
import{t as e}from"./rolldown-runtime-DK3Fl9T5.js";import{a as t,n,t as r}from"./api-DcdVQm8X.js";import{C as i,c as a,t as o,u as s}from"./index-BV8t_8Zo.js";import{T as c,_ as l,a as u,c as d,g as f,h as p,i as m,l as h,n as g,o as _,s as v,t as y,x as b,y as x}from"./gpu-data-evaluator-DowrXdaM.js";import{S,_ as C,b as w,h as T,p as E,x as D}from"./array-utils-flat-Bz_dufVy.js";import{a as O,c as k,i as A,l as j,o as ee,r as te,s as ne,t as re,u as ie}from"./webgl-device-McYZ27Ud.js";import{c as ae,d as oe,l as M,u as se}from"./get-attribute-from-layouts-DnJQzRbL.js";import{a as ce,i as le,n as ue,o as de,r as fe,s as pe,t as me}from"./buffer-transform-ZALaEdOc.js";import{n as he}from"./webgpu-yD2IvmEH.js";globalThis.probe={};var ge=new D({id:`@probe.gl/log`}),_e=`set luma.log.level=1 (or higher) to trace rendering`,ve="No matching device found. Ensure `@luma.gl/webgl` and/or `@luma.gl/webgpu` modules are imported.",ye=new class e{static defaultProps={...ie,type:`best-available`,adapters:void 0,waitForPageLoad:!0};stats=k;log=w;VERSION=`9.4.2`;spector;preregisteredAdapters=new Map;constructor(){if(globalThis.luma){if(globalThis.luma.VERSION!==this.VERSION)throw w.error(`Found luma.gl ${globalThis.luma.VERSION} while initialzing ${this.VERSION}`)(),w.error(`'yarn why @luma.gl/core' can help identify the source of the conflict`)(),Error(`luma.gl - multiple versions detected: see console log`);w.error(`This version of luma.gl has already been initialized`)()}w.log(1,`${this.VERSION} - ${_e}`)(),globalThis.luma=this}async createDevice(t={}){let n={...e.defaultProps,...t},r=this.selectAdapter(n.type,n.adapters);if(!r)throw Error(ve);return n.waitForPageLoad&&await r.pageLoaded,await r.create(n)}async attachDevice(e,t){let n=this._getTypeFromHandle(e,t.adapters),r=n&&this.selectAdapter(n,t.adapters);if(!r)throw Error(ve);return await r?.attach?.(e,t)}registerAdapters(e){for(let t of e)this.preregisteredAdapters.set(t.type,t)}getSupportedAdapters(e=[]){let t=this._getAdapterMap(e);return Array.from(t).map(([,e])=>e).filter(e=>e.isSupported?.()).map(e=>e.type)}getBestAvailableAdapterType(e=[]){let t=[`webgpu`,`webgl`,`null`],n=this._getAdapterMap(e);for(let e of t)if(n.get(e)?.isSupported?.())return e;return null}selectAdapter(e,t=[]){let n=e;e===`best-available`&&(n=this.getBestAvailableAdapterType(t));let r=this._getAdapterMap(t);return n&&r.get(n)||null}enforceWebGL2(e=!0,t=[]){let n=this._getAdapterMap(t).get(`webgl`);n||w.warn(`enforceWebGL2: webgl adapter not found`)(),n?.enforceWebGL2?.(e)}setDefaultDeviceProps(t){Object.assign(e.defaultProps,t)}_getAdapterMap(e=[]){let t=new Map(this.preregisteredAdapters);for(let n of e)t.set(n.type,n);return t}_getTypeFromHandle(e,t=[]){return e instanceof WebGL2RenderingContext?`webgl`:typeof GPUDevice<`u`&&e instanceof GPUDevice||e?.queue?`webgpu`:e===null?`null`:(e instanceof WebGLRenderingContext?w.warn(`WebGL1 is not supported`,e)():w.warn(`Unknown handle type`,e)(),null)}},be=class{get pageLoaded(){return we()}},xe=S()&&typeof document<`u`,Se=()=>xe&&document.readyState===`complete`,Ce=null;function we(){return Ce||=Se()||typeof window>`u`?Promise.resolve():new Promise(e=>window.addEventListener(`load`,()=>e())),Ce}var Te=1/Math.PI*180,Ee=1/180*Math.PI;globalThis.mathgl=globalThis.mathgl||{config:{EPSILON:1e-12,debug:!1,precision:4,printTypes:!1,printDegrees:!1,printRowMajor:!0,_cartographicRadians:!1}};var N=globalThis.mathgl.config;function De(e,{precision:t=N.precision}={}){return e=Fe(e),`${parseFloat(e.toPrecision(t))}`}function Oe(e){return Array.isArray(e)||ArrayBuffer.isView(e)&&!(e instanceof DataView)}function ke(e){return je(e)}function Ae(e){return Me(e)}function je(e,t){return Le(e,e=>e*Ee,t)}function Me(e,t){return Le(e,e=>e*Te,t)}function P(e,t,n){return Le(e,e=>Math.max(t,Math.min(n,e)))}function Ne(e,t,n){return Oe(e)?e.map((e,r)=>Ne(e,t[r],n)):n*t+(1-n)*e}function Pe(e,t,n){let r=N.EPSILON;n&&(N.EPSILON=n);try{if(e===t)return!0;if(Oe(e)&&Oe(t)){if(e.length!==t.length)return!1;for(let n=0;n<e.length;++n)if(!Pe(e[n],t[n]))return!1;return!0}return e&&e.equals?e.equals(t):t&&t.equals?t.equals(e):typeof e==`number`&&typeof t==`number`&&Math.abs(e-t)<=N.EPSILON*Math.max(1,Math.abs(e),Math.abs(t))}finally{N.EPSILON=r}}function Fe(e){return Math.round(e/N.EPSILON)*N.EPSILON}function Ie(e){return e.clone?e.clone():Array(e.length)}function Le(e,t,n){if(Oe(e)){let r=e;n||=Ie(r);for(let i=0;i<n.length&&i<r.length;++i){let r=typeof e==`number`?e:e[i];n[i]=t(r,i,n)}return n}return t(e)}var Re=class extends Array{clone(){return new this.constructor().copy(this)}fromArray(e,t=0){for(let n=0;n<this.ELEMENTS;++n)this[n]=e[n+t];return this.check()}toArray(e=[],t=0){for(let n=0;n<this.ELEMENTS;++n)e[t+n]=this[n];return e}toObject(e){return e}from(e){return Array.isArray(e)?this.copy(e):this.fromObject(e)}to(e){return e===this?this:Oe(e)?this.toArray(e):this.toObject(e)}toTarget(e){return e?this.to(e):this}toFloat32Array(){return new Float32Array(this)}toString(){return this.formatString(N)}formatString(e){let t=``;for(let n=0;n<this.ELEMENTS;++n)t+=(n>0?`, `:``)+De(this[n],e);return`${e.printTypes?this.constructor.name:``}[${t}]`}equals(e){if(!e||this.length!==e.length)return!1;for(let t=0;t<this.ELEMENTS;++t)if(!Pe(this[t],e[t]))return!1;return!0}exactEquals(e){if(!e||this.length!==e.length)return!1;for(let t=0;t<this.ELEMENTS;++t)if(this[t]!==e[t])return!1;return!0}negate(){for(let e=0;e<this.ELEMENTS;++e)this[e]=-this[e];return this.check()}lerp(e,t,n){if(n===void 0)return this.lerp(this,e,t);for(let r=0;r<this.ELEMENTS;++r){let i=e[r],a=typeof t==`number`?t:t[r];this[r]=i+n*(a-i)}return this.check()}min(e){for(let t=0;t<this.ELEMENTS;++t)this[t]=Math.min(e[t],this[t]);return this.check()}max(e){for(let t=0;t<this.ELEMENTS;++t)this[t]=Math.max(e[t],this[t]);return this.check()}clamp(e,t){for(let n=0;n<this.ELEMENTS;++n)this[n]=Math.min(Math.max(this[n],e[n]),t[n]);return this.check()}add(...e){for(let t of e)for(let e=0;e<this.ELEMENTS;++e)this[e]+=t[e];return this.check()}subtract(...e){for(let t of e)for(let e=0;e<this.ELEMENTS;++e)this[e]-=t[e];return this.check()}scale(e){if(typeof e==`number`)for(let t=0;t<this.ELEMENTS;++t)this[t]*=e;else for(let t=0;t<this.ELEMENTS&&t<e.length;++t)this[t]*=e[t];return this.check()}multiplyByScalar(e){for(let t=0;t<this.ELEMENTS;++t)this[t]*=e;return this.check()}check(){if(N.debug&&!this.validate())throw Error(`math.gl: ${this.constructor.name} some fields set to invalid numbers'`);return this}validate(){let e=this.length===this.ELEMENTS;for(let t=0;t<this.ELEMENTS;++t)e&&=Number.isFinite(this[t]);return e}sub(e){return this.subtract(e)}setScalar(e){for(let t=0;t<this.ELEMENTS;++t)this[t]=e;return this.check()}addScalar(e){for(let t=0;t<this.ELEMENTS;++t)this[t]+=e;return this.check()}subScalar(e){return this.addScalar(-e)}multiplyScalar(e){for(let t=0;t<this.ELEMENTS;++t)this[t]*=e;return this.check()}divideScalar(e){return this.multiplyByScalar(1/e)}clampScalar(e,t){for(let n=0;n<this.ELEMENTS;++n)this[n]=Math.min(Math.max(this[n],e),t);return this.check()}get elements(){return this}};function ze(e,t){if(e.length!==t)return!1;for(let t=0;t<e.length;++t)if(!Number.isFinite(e[t]))return!1;return!0}function F(e){if(!Number.isFinite(e))throw Error(`Invalid number ${JSON.stringify(e)}`);return e}function Be(e,t,n=``){if(N.debug&&!ze(e,t))throw Error(`math.gl: ${n} some fields set to invalid numbers'`);return e}function Ve(e,t){if(!e)throw Error(`math.gl assertion ${t}`)}var He=class extends Re{get x(){return this[0]}set x(e){this[0]=F(e)}get y(){return this[1]}set y(e){this[1]=F(e)}len(){return Math.sqrt(this.lengthSquared())}magnitude(){return this.len()}lengthSquared(){let e=0;for(let t=0;t<this.ELEMENTS;++t)e+=this[t]*this[t];return e}magnitudeSquared(){return this.lengthSquared()}distance(e){return Math.sqrt(this.distanceSquared(e))}distanceSquared(e){let t=0;for(let n=0;n<this.ELEMENTS;++n){let r=this[n]-e[n];t+=r*r}return F(t)}dot(e){let t=0;for(let n=0;n<this.ELEMENTS;++n)t+=this[n]*e[n];return F(t)}normalize(){let e=this.magnitude();if(e!==0)for(let t=0;t<this.ELEMENTS;++t)this[t]/=e;return this.check()}multiply(...e){for(let t of e)for(let e=0;e<this.ELEMENTS;++e)this[e]*=t[e];return this.check()}divide(...e){for(let t of e)for(let e=0;e<this.ELEMENTS;++e)this[e]/=t[e];return this.check()}lengthSq(){return this.lengthSquared()}distanceTo(e){return this.distance(e)}distanceToSquared(e){return this.distanceSquared(e)}getComponent(e){return Ve(e>=0&&e<this.ELEMENTS,`index is out of range`),F(this[e])}setComponent(e,t){return Ve(e>=0&&e<this.ELEMENTS,`index is out of range`),this[e]=t,this.check()}addVectors(e,t){return this.copy(e).add(t)}subVectors(e,t){return this.copy(e).subtract(t)}multiplyVectors(e,t){return this.copy(e).multiply(t)}addScaledVector(e,t){return this.add(new this.constructor(e).multiplyScalar(t))}},Ue=typeof Float32Array<`u`?Float32Array:Array;Math.PI/180;function We(){let e=new Ue(2);return Ue!=Float32Array&&(e[0]=0,e[1]=0),e}function Ge(e,t,n){return e[0]=t[0]+n[0],e[1]=t[1]+n[1],e}function Ke(e,t,n){return e[0]=t[0]-n[0],e[1]=t[1]-n[1],e}function qe(e,t){return e[0]=-t[0],e[1]=-t[1],e}function Je(e,t,n,r){let i=t[0],a=t[1];return e[0]=i+r*(n[0]-i),e[1]=a+r*(n[1]-a),e}function Ye(e,t,n){let r=t[0],i=t[1];return e[0]=n[0]*r+n[2]*i,e[1]=n[1]*r+n[3]*i,e}function Xe(e,t,n){let r=t[0],i=t[1];return e[0]=n[0]*r+n[2]*i+n[4],e[1]=n[1]*r+n[3]*i+n[5],e}function Ze(e,t,n){let r=t[0],i=t[1];return e[0]=n[0]*r+n[3]*i+n[6],e[1]=n[1]*r+n[4]*i+n[7],e}function Qe(e,t,n){let r=t[0],i=t[1];return e[0]=n[0]*r+n[4]*i+n[12],e[1]=n[1]*r+n[5]*i+n[13],e}var $e=Ke;(function(){let e=We();return function(t,n,r,i,a,o){let s,c;for(n||=2,r||=0,c=i?Math.min(i*n+r,t.length):t.length,s=r;s<c;s+=n)e[0]=t[s],e[1]=t[s+1],a(e,e,o),t[s]=e[0],t[s+1]=e[1];return t}})();function et(e,t,n){let r=t[0],i=t[1],a=n[3]*r+n[7]*i||1;return e[0]=(n[0]*r+n[4]*i)/a,e[1]=(n[1]*r+n[5]*i)/a,e}function tt(e,t,n){let r=t[0],i=t[1],a=t[2],o=n[3]*r+n[7]*i+n[11]*a||1;return e[0]=(n[0]*r+n[4]*i+n[8]*a)/o,e[1]=(n[1]*r+n[5]*i+n[9]*a)/o,e[2]=(n[2]*r+n[6]*i+n[10]*a)/o,e}function nt(e,t,n){let r=t[0],i=t[1];return e[0]=n[0]*r+n[2]*i,e[1]=n[1]*r+n[3]*i,e[2]=t[2],e}function rt(e,t,n){let r=t[0],i=t[1];return e[0]=n[0]*r+n[2]*i,e[1]=n[1]*r+n[3]*i,e[2]=t[2],e[3]=t[3],e}function it(e,t,n){let r=t[0],i=t[1],a=t[2];return e[0]=n[0]*r+n[3]*i+n[6]*a,e[1]=n[1]*r+n[4]*i+n[7]*a,e[2]=n[2]*r+n[5]*i+n[8]*a,e[3]=t[3],e}var at=class extends He{constructor(e=0,t=0){super(2),Oe(e)&&arguments.length===1?this.copy(e):(N.debug&&(F(e),F(t)),this[0]=e,this[1]=t)}set(e,t){return this[0]=e,this[1]=t,this.check()}copy(e){return this[0]=e[0],this[1]=e[1],this.check()}fromObject(e){return N.debug&&(F(e.x),F(e.y)),this[0]=e.x,this[1]=e.y,this.check()}toObject(e){return e.x=this[0],e.y=this[1],e}get ELEMENTS(){return 2}horizontalAngle(){return Math.atan2(this.y,this.x)}verticalAngle(){return Math.atan2(this.x,this.y)}transform(e){return this.transformAsPoint(e)}transformAsPoint(e){return Qe(this,this,e),this.check()}transformAsVector(e){return et(this,this,e),this.check()}transformByMatrix3(e){return Ze(this,this,e),this.check()}transformByMatrix2x3(e){return Xe(this,this,e),this.check()}transformByMatrix2(e){return Ye(this,this,e),this.check()}};function ot(){let e=new Ue(3);return Ue!=Float32Array&&(e[0]=0,e[1]=0,e[2]=0),e}function st(e){let t=e[0],n=e[1],r=e[2];return Math.sqrt(t*t+n*n+r*r)}function ct(e,t,n){let r=new Ue(3);return r[0]=e,r[1]=t,r[2]=n,r}function lt(e,t,n){return e[0]=t[0]-n[0],e[1]=t[1]-n[1],e[2]=t[2]-n[2],e}function ut(e){let t=e[0],n=e[1],r=e[2];return t*t+n*n+r*r}function dt(e,t){return e[0]=-t[0],e[1]=-t[1],e[2]=-t[2],e}function ft(e,t){let n=t[0],r=t[1],i=t[2],a=n*n+r*r+i*i;return a>0&&(a=1/Math.sqrt(a)),e[0]=t[0]*a,e[1]=t[1]*a,e[2]=t[2]*a,e}function pt(e,t){return e[0]*t[0]+e[1]*t[1]+e[2]*t[2]}function mt(e,t,n){let r=t[0],i=t[1],a=t[2],o=n[0],s=n[1],c=n[2];return e[0]=i*c-a*s,e[1]=a*o-r*c,e[2]=r*s-i*o,e}function ht(e,t,n,r){let i=t[0],a=t[1],o=t[2];return e[0]=i+r*(n[0]-i),e[1]=a+r*(n[1]-a),e[2]=o+r*(n[2]-o),e}function gt(e,t,n){let r=t[0],i=t[1],a=t[2],o=n[3]*r+n[7]*i+n[11]*a+n[15];return o||=1,e[0]=(n[0]*r+n[4]*i+n[8]*a+n[12])/o,e[1]=(n[1]*r+n[5]*i+n[9]*a+n[13])/o,e[2]=(n[2]*r+n[6]*i+n[10]*a+n[14])/o,e}function _t(e,t,n){let r=t[0],i=t[1],a=t[2];return e[0]=r*n[0]+i*n[3]+a*n[6],e[1]=r*n[1]+i*n[4]+a*n[7],e[2]=r*n[2]+i*n[5]+a*n[8],e}function vt(e,t,n){let r=n[0],i=n[1],a=n[2],o=n[3],s=t[0],c=t[1],l=t[2],u=i*l-a*c,d=a*s-r*l,f=r*c-i*s,p=i*f-a*d,m=a*u-r*f,h=r*d-i*u,g=o*2;return u*=g,d*=g,f*=g,p*=2,m*=2,h*=2,e[0]=s+u+p,e[1]=c+d+m,e[2]=l+f+h,e}function yt(e,t,n,r){let i=[],a=[];return i[0]=t[0]-n[0],i[1]=t[1]-n[1],i[2]=t[2]-n[2],a[0]=i[0],a[1]=i[1]*Math.cos(r)-i[2]*Math.sin(r),a[2]=i[1]*Math.sin(r)+i[2]*Math.cos(r),e[0]=a[0]+n[0],e[1]=a[1]+n[1],e[2]=a[2]+n[2],e}function bt(e,t,n,r){let i=[],a=[];return i[0]=t[0]-n[0],i[1]=t[1]-n[1],i[2]=t[2]-n[2],a[0]=i[2]*Math.sin(r)+i[0]*Math.cos(r),a[1]=i[1],a[2]=i[2]*Math.cos(r)-i[0]*Math.sin(r),e[0]=a[0]+n[0],e[1]=a[1]+n[1],e[2]=a[2]+n[2],e}function xt(e,t,n,r){let i=[],a=[];return i[0]=t[0]-n[0],i[1]=t[1]-n[1],i[2]=t[2]-n[2],a[0]=i[0]*Math.cos(r)-i[1]*Math.sin(r),a[1]=i[0]*Math.sin(r)+i[1]*Math.cos(r),a[2]=i[2],e[0]=a[0]+n[0],e[1]=a[1]+n[1],e[2]=a[2]+n[2],e}function St(e,t){let n=e[0],r=e[1],i=e[2],a=t[0],o=t[1],s=t[2],c=Math.sqrt((n*n+r*r+i*i)*(a*a+o*o+s*s)),l=c&&pt(e,t)/c;return Math.acos(Math.min(Math.max(l,-1),1))}var Ct=lt,wt=st,Tt=ut;(function(){let e=ot();return function(t,n,r,i,a,o){let s,c;for(n||=3,r||=0,c=i?Math.min(i*n+r,t.length):t.length,s=r;s<c;s+=n)e[0]=t[s],e[1]=t[s+1],e[2]=t[s+2],a(e,e,o),t[s]=e[0],t[s+1]=e[1],t[s+2]=e[2];return t}})();var Et=[0,0,0],Dt,I=class e extends He{static get ZERO(){return Dt||(Dt=new e(0,0,0),Object.freeze(Dt)),Dt}constructor(e=0,t=0,n=0){super(-0,-0,-0),arguments.length===1&&Oe(e)?this.copy(e):(N.debug&&(F(e),F(t),F(n)),this[0]=e,this[1]=t,this[2]=n)}set(e,t,n){return this[0]=e,this[1]=t,this[2]=n,this.check()}copy(e){return this[0]=e[0],this[1]=e[1],this[2]=e[2],this.check()}fromObject(e){return N.debug&&(F(e.x),F(e.y),F(e.z)),this[0]=e.x,this[1]=e.y,this[2]=e.z,this.check()}toObject(e){return e.x=this[0],e.y=this[1],e.z=this[2],e}get ELEMENTS(){return 3}get z(){return this[2]}set z(e){this[2]=F(e)}angle(e){return St(this,e)}cross(e){return mt(this,this,e),this.check()}rotateX({radians:e,origin:t=Et}){return yt(this,this,t,e),this.check()}rotateY({radians:e,origin:t=Et}){return bt(this,this,t,e),this.check()}rotateZ({radians:e,origin:t=Et}){return xt(this,this,t,e),this.check()}transform(e){return this.transformAsPoint(e)}transformAsPoint(e){return gt(this,this,e),this.check()}transformAsVector(e){return tt(this,this,e),this.check()}transformByMatrix3(e){return _t(this,this,e),this.check()}transformByMatrix2(e){return nt(this,this,e),this.check()}transformByQuaternion(e){return vt(this,this,e),this.check()}},Ot,kt=class e extends He{static get ZERO(){return Ot||(Ot=new e(0,0,0,0),Object.freeze(Ot)),Ot}constructor(e=0,t=0,n=0,r=0){super(-0,-0,-0,-0),Oe(e)&&arguments.length===1?this.copy(e):(N.debug&&(F(e),F(t),F(n),F(r)),this[0]=e,this[1]=t,this[2]=n,this[3]=r)}set(e,t,n,r){return this[0]=e,this[1]=t,this[2]=n,this[3]=r,this.check()}copy(e){return this[0]=e[0],this[1]=e[1],this[2]=e[2],this[3]=e[3],this.check()}fromObject(e){return N.debug&&(F(e.x),F(e.y),F(e.z),F(e.w)),this[0]=e.x,this[1]=e.y,this[2]=e.z,this[3]=e.w,this}toObject(e){return e.x=this[0],e.y=this[1],e.z=this[2],e.w=this[3],e}get ELEMENTS(){return 4}get z(){return this[2]}set z(e){this[2]=F(e)}get w(){return this[3]}set w(e){this[3]=F(e)}transform(e){return gt(this,this,e),this.check()}transformByMatrix3(e){return it(this,this,e),this.check()}transformByMatrix2(e){return rt(this,this,e),this.check()}transformByQuaternion(e){return vt(this,this,e),this.check()}applyMatrix4(e){return e.transform(this,this),this}},At=class extends Re{toString(){let e=`[`;if(N.printRowMajor){e+=`row-major:`;for(let t=0;t<this.RANK;++t)for(let n=0;n<this.RANK;++n)e+=` ${this[n*this.RANK+t]}`}else{e+=`column-major:`;for(let t=0;t<this.ELEMENTS;++t)e+=` ${this[t]}`}return e+=`]`,e}getElementIndex(e,t){return t*this.RANK+e}getElement(e,t){return this[t*this.RANK+e]}setElement(e,t,n){return this[t*this.RANK+e]=F(n),this}getColumn(e,t=Array(this.RANK).fill(-0)){let n=e*this.RANK;for(let e=0;e<this.RANK;++e)t[e]=this[n+e];return t}setColumn(e,t){let n=e*this.RANK;for(let e=0;e<this.RANK;++e)this[n+e]=t[e];return this}};function jt(){let e=new Ue(9);return Ue!=Float32Array&&(e[1]=0,e[2]=0,e[3]=0,e[5]=0,e[6]=0,e[7]=0),e[0]=1,e[4]=1,e[8]=1,e}function Mt(e,t){if(e===t){let n=t[1],r=t[2],i=t[5];e[1]=t[3],e[2]=t[6],e[3]=n,e[5]=t[7],e[6]=r,e[7]=i}else e[0]=t[0],e[1]=t[3],e[2]=t[6],e[3]=t[1],e[4]=t[4],e[5]=t[7],e[6]=t[2],e[7]=t[5],e[8]=t[8];return e}function Nt(e,t){let n=t[0],r=t[1],i=t[2],a=t[3],o=t[4],s=t[5],c=t[6],l=t[7],u=t[8],d=u*o-s*l,f=-u*a+s*c,p=l*a-o*c,m=n*d+r*f+i*p;return m?(m=1/m,e[0]=d*m,e[1]=(-u*r+i*l)*m,e[2]=(s*r-i*o)*m,e[3]=f*m,e[4]=(u*n-i*c)*m,e[5]=(-s*n+i*a)*m,e[6]=p*m,e[7]=(-l*n+r*c)*m,e[8]=(o*n-r*a)*m,e):null}function Pt(e){let t=e[0],n=e[1],r=e[2],i=e[3],a=e[4],o=e[5],s=e[6],c=e[7],l=e[8];return t*(l*a-o*c)+n*(-l*i+o*s)+r*(c*i-a*s)}function Ft(e,t,n){let r=t[0],i=t[1],a=t[2],o=t[3],s=t[4],c=t[5],l=t[6],u=t[7],d=t[8],f=n[0],p=n[1],m=n[2],h=n[3],g=n[4],_=n[5],v=n[6],y=n[7],b=n[8];return e[0]=f*r+p*o+m*l,e[1]=f*i+p*s+m*u,e[2]=f*a+p*c+m*d,e[3]=h*r+g*o+_*l,e[4]=h*i+g*s+_*u,e[5]=h*a+g*c+_*d,e[6]=v*r+y*o+b*l,e[7]=v*i+y*s+b*u,e[8]=v*a+y*c+b*d,e}function It(e,t,n){let r=t[0],i=t[1],a=t[2],o=t[3],s=t[4],c=t[5],l=t[6],u=t[7],d=t[8],f=n[0],p=n[1];return e[0]=r,e[1]=i,e[2]=a,e[3]=o,e[4]=s,e[5]=c,e[6]=f*r+p*o+l,e[7]=f*i+p*s+u,e[8]=f*a+p*c+d,e}function Lt(e,t,n){let r=t[0],i=t[1],a=t[2],o=t[3],s=t[4],c=t[5],l=t[6],u=t[7],d=t[8],f=Math.sin(n),p=Math.cos(n);return e[0]=p*r+f*o,e[1]=p*i+f*s,e[2]=p*a+f*c,e[3]=p*o-f*r,e[4]=p*s-f*i,e[5]=p*c-f*a,e[6]=l,e[7]=u,e[8]=d,e}function Rt(e,t,n){let r=n[0],i=n[1];return e[0]=r*t[0],e[1]=r*t[1],e[2]=r*t[2],e[3]=i*t[3],e[4]=i*t[4],e[5]=i*t[5],e[6]=t[6],e[7]=t[7],e[8]=t[8],e}function zt(e,t){let n=t[0],r=t[1],i=t[2],a=t[3],o=n+n,s=r+r,c=i+i,l=n*o,u=r*o,d=r*s,f=i*o,p=i*s,m=i*c,h=a*o,g=a*s,_=a*c;return e[0]=1-d-m,e[3]=u-_,e[6]=f+g,e[1]=u+_,e[4]=1-l-m,e[7]=p-h,e[2]=f-g,e[5]=p+h,e[8]=1-l-d,e}var Bt;(function(e){e[e.COL0ROW0=0]=`COL0ROW0`,e[e.COL0ROW1=1]=`COL0ROW1`,e[e.COL0ROW2=2]=`COL0ROW2`,e[e.COL1ROW0=3]=`COL1ROW0`,e[e.COL1ROW1=4]=`COL1ROW1`,e[e.COL1ROW2=5]=`COL1ROW2`,e[e.COL2ROW0=6]=`COL2ROW0`,e[e.COL2ROW1=7]=`COL2ROW1`,e[e.COL2ROW2=8]=`COL2ROW2`})(Bt||={});var Vt=Object.freeze([1,0,0,0,1,0,0,0,1]),L=class extends At{static get IDENTITY(){return Gt()}static get ZERO(){return Wt()}get ELEMENTS(){return 9}get RANK(){return 3}get INDICES(){return Bt}constructor(e,...t){super(-0,-0,-0,-0,-0,-0,-0,-0,-0),arguments.length===1&&Array.isArray(e)?this.copy(e):t.length>0?this.copy([e,...t]):this.identity()}copy(e){return this[0]=e[0],this[1]=e[1],this[2]=e[2],this[3]=e[3],this[4]=e[4],this[5]=e[5],this[6]=e[6],this[7]=e[7],this[8]=e[8],this.check()}identity(){return this.copy(Vt)}fromObject(e){return this.check()}fromQuaternion(e){return zt(this,e),this.check()}set(e,t,n,r,i,a,o,s,c){return this[0]=e,this[1]=t,this[2]=n,this[3]=r,this[4]=i,this[5]=a,this[6]=o,this[7]=s,this[8]=c,this.check()}setRowMajor(e,t,n,r,i,a,o,s,c){return this[0]=e,this[1]=r,this[2]=o,this[3]=t,this[4]=i,this[5]=s,this[6]=n,this[7]=a,this[8]=c,this.check()}determinant(){return Pt(this)}transpose(){return Mt(this,this),this.check()}invert(){return Nt(this,this),this.check()}multiplyLeft(e){return Ft(this,e,this),this.check()}multiplyRight(e){return Ft(this,this,e),this.check()}rotate(e){return Lt(this,this,e),this.check()}scale(e){return Array.isArray(e)?Rt(this,this,e):Rt(this,this,[e,e]),this.check()}translate(e){return It(this,this,e),this.check()}transform(e,t){let n;switch(e.length){case 2:n=Ze(t||[-0,-0],e,this);break;case 3:n=_t(t||[-0,-0,-0],e,this);break;case 4:n=it(t||[-0,-0,-0,-0],e,this);break;default:throw Error(`Illegal vector`)}return Be(n,e.length),n}transformVector(e,t){return this.transform(e,t)}transformVector2(e,t){return this.transform(e,t)}transformVector3(e,t){return this.transform(e,t)}},Ht,Ut=null;function Wt(){return Ht||(Ht=new L([0,0,0,0,0,0,0,0,0]),Object.freeze(Ht)),Ht}function Gt(){return Ut||(Ut=new L,Object.freeze(Ut)),Ut}function Kt(e){return e[0]=1,e[1]=0,e[2]=0,e[3]=0,e[4]=0,e[5]=1,e[6]=0,e[7]=0,e[8]=0,e[9]=0,e[10]=1,e[11]=0,e[12]=0,e[13]=0,e[14]=0,e[15]=1,e}function qt(e,t){if(e===t){let n=t[1],r=t[2],i=t[3],a=t[6],o=t[7],s=t[11];e[1]=t[4],e[2]=t[8],e[3]=t[12],e[4]=n,e[6]=t[9],e[7]=t[13],e[8]=r,e[9]=a,e[11]=t[14],e[12]=i,e[13]=o,e[14]=s}else e[0]=t[0],e[1]=t[4],e[2]=t[8],e[3]=t[12],e[4]=t[1],e[5]=t[5],e[6]=t[9],e[7]=t[13],e[8]=t[2],e[9]=t[6],e[10]=t[10],e[11]=t[14],e[12]=t[3],e[13]=t[7],e[14]=t[11],e[15]=t[15];return e}function Jt(e,t){let n=t[0],r=t[1],i=t[2],a=t[3],o=t[4],s=t[5],c=t[6],l=t[7],u=t[8],d=t[9],f=t[10],p=t[11],m=t[12],h=t[13],g=t[14],_=t[15],v=n*s-r*o,y=n*c-i*o,b=n*l-a*o,x=r*c-i*s,S=r*l-a*s,C=i*l-a*c,w=u*h-d*m,T=u*g-f*m,E=u*_-p*m,D=d*g-f*h,O=d*_-p*h,k=f*_-p*g,A=v*k-y*O+b*D+x*E-S*T+C*w;return A?(A=1/A,e[0]=(s*k-c*O+l*D)*A,e[1]=(i*O-r*k-a*D)*A,e[2]=(h*C-g*S+_*x)*A,e[3]=(f*S-d*C-p*x)*A,e[4]=(c*E-o*k-l*T)*A,e[5]=(n*k-i*E+a*T)*A,e[6]=(g*b-m*C-_*y)*A,e[7]=(u*C-f*b+p*y)*A,e[8]=(o*O-s*E+l*w)*A,e[9]=(r*E-n*O-a*w)*A,e[10]=(m*S-h*b+_*v)*A,e[11]=(d*b-u*S-p*v)*A,e[12]=(s*T-o*D-c*w)*A,e[13]=(n*D-r*T+i*w)*A,e[14]=(h*y-m*x-g*v)*A,e[15]=(u*x-d*y+f*v)*A,e):null}function Yt(e){let t=e[0],n=e[1],r=e[2],i=e[3],a=e[4],o=e[5],s=e[6],c=e[7],l=e[8],u=e[9],d=e[10],f=e[11],p=e[12],m=e[13],h=e[14],g=e[15],_=t*o-n*a,v=t*s-r*a,y=n*s-r*o,b=l*m-u*p,x=l*h-d*p,S=u*h-d*m,C=t*S-n*x+r*b,w=a*S-o*x+s*b,T=l*y-u*v+d*_,E=p*y-m*v+h*_;return c*C-i*w+g*T-f*E}function Xt(e,t,n){let r=t[0],i=t[1],a=t[2],o=t[3],s=t[4],c=t[5],l=t[6],u=t[7],d=t[8],f=t[9],p=t[10],m=t[11],h=t[12],g=t[13],_=t[14],v=t[15],y=n[0],b=n[1],x=n[2],S=n[3];return e[0]=y*r+b*s+x*d+S*h,e[1]=y*i+b*c+x*f+S*g,e[2]=y*a+b*l+x*p+S*_,e[3]=y*o+b*u+x*m+S*v,y=n[4],b=n[5],x=n[6],S=n[7],e[4]=y*r+b*s+x*d+S*h,e[5]=y*i+b*c+x*f+S*g,e[6]=y*a+b*l+x*p+S*_,e[7]=y*o+b*u+x*m+S*v,y=n[8],b=n[9],x=n[10],S=n[11],e[8]=y*r+b*s+x*d+S*h,e[9]=y*i+b*c+x*f+S*g,e[10]=y*a+b*l+x*p+S*_,e[11]=y*o+b*u+x*m+S*v,y=n[12],b=n[13],x=n[14],S=n[15],e[12]=y*r+b*s+x*d+S*h,e[13]=y*i+b*c+x*f+S*g,e[14]=y*a+b*l+x*p+S*_,e[15]=y*o+b*u+x*m+S*v,e}function Zt(e,t,n){let r=n[0],i=n[1],a=n[2],o,s,c,l,u,d,f,p,m,h,g,_;return t===e?(e[12]=t[0]*r+t[4]*i+t[8]*a+t[12],e[13]=t[1]*r+t[5]*i+t[9]*a+t[13],e[14]=t[2]*r+t[6]*i+t[10]*a+t[14],e[15]=t[3]*r+t[7]*i+t[11]*a+t[15]):(o=t[0],s=t[1],c=t[2],l=t[3],u=t[4],d=t[5],f=t[6],p=t[7],m=t[8],h=t[9],g=t[10],_=t[11],e[0]=o,e[1]=s,e[2]=c,e[3]=l,e[4]=u,e[5]=d,e[6]=f,e[7]=p,e[8]=m,e[9]=h,e[10]=g,e[11]=_,e[12]=o*r+u*i+m*a+t[12],e[13]=s*r+d*i+h*a+t[13],e[14]=c*r+f*i+g*a+t[14],e[15]=l*r+p*i+_*a+t[15]),e}function Qt(e,t,n){let r=n[0],i=n[1],a=n[2];return e[0]=t[0]*r,e[1]=t[1]*r,e[2]=t[2]*r,e[3]=t[3]*r,e[4]=t[4]*i,e[5]=t[5]*i,e[6]=t[6]*i,e[7]=t[7]*i,e[8]=t[8]*a,e[9]=t[9]*a,e[10]=t[10]*a,e[11]=t[11]*a,e[12]=t[12],e[13]=t[13],e[14]=t[14],e[15]=t[15],e}function $t(e,t,n,r){let i=r[0],a=r[1],o=r[2],s=Math.sqrt(i*i+a*a+o*o),c,l,u,d,f,p,m,h,g,_,v,y,b,x,S,C,w,T,E,D,O,k,A,j;return s<1e-6?null:(s=1/s,i*=s,a*=s,o*=s,l=Math.sin(n),c=Math.cos(n),u=1-c,d=t[0],f=t[1],p=t[2],m=t[3],h=t[4],g=t[5],_=t[6],v=t[7],y=t[8],b=t[9],x=t[10],S=t[11],C=i*i*u+c,w=a*i*u+o*l,T=o*i*u-a*l,E=i*a*u-o*l,D=a*a*u+c,O=o*a*u+i*l,k=i*o*u+a*l,A=a*o*u-i*l,j=o*o*u+c,e[0]=d*C+h*w+y*T,e[1]=f*C+g*w+b*T,e[2]=p*C+_*w+x*T,e[3]=m*C+v*w+S*T,e[4]=d*E+h*D+y*O,e[5]=f*E+g*D+b*O,e[6]=p*E+_*D+x*O,e[7]=m*E+v*D+S*O,e[8]=d*k+h*A+y*j,e[9]=f*k+g*A+b*j,e[10]=p*k+_*A+x*j,e[11]=m*k+v*A+S*j,t!==e&&(e[12]=t[12],e[13]=t[13],e[14]=t[14],e[15]=t[15]),e)}function en(e,t,n){let r=Math.sin(n),i=Math.cos(n),a=t[4],o=t[5],s=t[6],c=t[7],l=t[8],u=t[9],d=t[10],f=t[11];return t!==e&&(e[0]=t[0],e[1]=t[1],e[2]=t[2],e[3]=t[3],e[12]=t[12],e[13]=t[13],e[14]=t[14],e[15]=t[15]),e[4]=a*i+l*r,e[5]=o*i+u*r,e[6]=s*i+d*r,e[7]=c*i+f*r,e[8]=l*i-a*r,e[9]=u*i-o*r,e[10]=d*i-s*r,e[11]=f*i-c*r,e}function tn(e,t,n){let r=Math.sin(n),i=Math.cos(n),a=t[0],o=t[1],s=t[2],c=t[3],l=t[8],u=t[9],d=t[10],f=t[11];return t!==e&&(e[4]=t[4],e[5]=t[5],e[6]=t[6],e[7]=t[7],e[12]=t[12],e[13]=t[13],e[14]=t[14],e[15]=t[15]),e[0]=a*i-l*r,e[1]=o*i-u*r,e[2]=s*i-d*r,e[3]=c*i-f*r,e[8]=a*r+l*i,e[9]=o*r+u*i,e[10]=s*r+d*i,e[11]=c*r+f*i,e}function nn(e,t,n){let r=Math.sin(n),i=Math.cos(n),a=t[0],o=t[1],s=t[2],c=t[3],l=t[4],u=t[5],d=t[6],f=t[7];return t!==e&&(e[8]=t[8],e[9]=t[9],e[10]=t[10],e[11]=t[11],e[12]=t[12],e[13]=t[13],e[14]=t[14],e[15]=t[15]),e[0]=a*i+l*r,e[1]=o*i+u*r,e[2]=s*i+d*r,e[3]=c*i+f*r,e[4]=l*i-a*r,e[5]=u*i-o*r,e[6]=d*i-s*r,e[7]=f*i-c*r,e}function rn(e,t){let n=t[0],r=t[1],i=t[2],a=t[4],o=t[5],s=t[6],c=t[8],l=t[9],u=t[10];return e[0]=Math.sqrt(n*n+r*r+i*i),e[1]=Math.sqrt(a*a+o*o+s*s),e[2]=Math.sqrt(c*c+l*l+u*u),e}function an(e,t){let n=t[0],r=t[1],i=t[2],a=t[3],o=n+n,s=r+r,c=i+i,l=n*o,u=r*o,d=r*s,f=i*o,p=i*s,m=i*c,h=a*o,g=a*s,_=a*c;return e[0]=1-d-m,e[1]=u+_,e[2]=f-g,e[3]=0,e[4]=u-_,e[5]=1-l-m,e[6]=p+h,e[7]=0,e[8]=f+g,e[9]=p-h,e[10]=1-l-d,e[11]=0,e[12]=0,e[13]=0,e[14]=0,e[15]=1,e}function on(e,t,n,r,i,a,o){let s=1/(n-t),c=1/(i-r),l=1/(a-o);return e[0]=a*2*s,e[1]=0,e[2]=0,e[3]=0,e[4]=0,e[5]=a*2*c,e[6]=0,e[7]=0,e[8]=(n+t)*s,e[9]=(i+r)*c,e[10]=(o+a)*l,e[11]=-1,e[12]=0,e[13]=0,e[14]=o*a*2*l,e[15]=0,e}function sn(e,t,n,r,i){let a=1/Math.tan(t/2);if(e[0]=a/n,e[1]=0,e[2]=0,e[3]=0,e[4]=0,e[5]=a,e[6]=0,e[7]=0,e[8]=0,e[9]=0,e[11]=-1,e[12]=0,e[13]=0,e[15]=0,i!=null&&i!==1/0){let t=1/(r-i);e[10]=(i+r)*t,e[14]=2*i*r*t}else e[10]=-1,e[14]=-2*r;return e}var cn=sn;function ln(e,t,n,r,i,a,o){let s=1/(t-n),c=1/(r-i),l=1/(a-o);return e[0]=-2*s,e[1]=0,e[2]=0,e[3]=0,e[4]=0,e[5]=-2*c,e[6]=0,e[7]=0,e[8]=0,e[9]=0,e[10]=2*l,e[11]=0,e[12]=(t+n)*s,e[13]=(i+r)*c,e[14]=(o+a)*l,e[15]=1,e}var un=ln;function dn(e,t,n,r){let i,a,o,s,c,l,u,d,f,p,m=t[0],h=t[1],g=t[2],_=r[0],v=r[1],y=r[2],b=n[0],x=n[1],S=n[2];return Math.abs(m-b)<1e-6&&Math.abs(h-x)<1e-6&&Math.abs(g-S)<1e-6?Kt(e):(d=m-b,f=h-x,p=g-S,i=1/Math.sqrt(d*d+f*f+p*p),d*=i,f*=i,p*=i,a=v*p-y*f,o=y*d-_*p,s=_*f-v*d,i=Math.sqrt(a*a+o*o+s*s),i?(i=1/i,a*=i,o*=i,s*=i):(a=0,o=0,s=0),c=f*s-p*o,l=p*a-d*s,u=d*o-f*a,i=Math.sqrt(c*c+l*l+u*u),i?(i=1/i,c*=i,l*=i,u*=i):(c=0,l=0,u=0),e[0]=a,e[1]=c,e[2]=d,e[3]=0,e[4]=o,e[5]=l,e[6]=f,e[7]=0,e[8]=s,e[9]=u,e[10]=p,e[11]=0,e[12]=-(a*m+o*h+s*g),e[13]=-(c*m+l*h+u*g),e[14]=-(d*m+f*h+p*g),e[15]=1,e)}function fn(){let e=new Ue(4);return Ue!=Float32Array&&(e[0]=0,e[1]=0,e[2]=0,e[3]=0),e}function pn(e,t,n){return e[0]=t[0]+n[0],e[1]=t[1]+n[1],e[2]=t[2]+n[2],e[3]=t[3]+n[3],e}function mn(e,t,n){return e[0]=t[0]*n,e[1]=t[1]*n,e[2]=t[2]*n,e[3]=t[3]*n,e}function hn(e){let t=e[0],n=e[1],r=e[2],i=e[3];return Math.sqrt(t*t+n*n+r*r+i*i)}function gn(e){let t=e[0],n=e[1],r=e[2],i=e[3];return t*t+n*n+r*r+i*i}function _n(e,t){let n=t[0],r=t[1],i=t[2],a=t[3],o=n*n+r*r+i*i+a*a;return o>0&&(o=1/Math.sqrt(o)),e[0]=n*o,e[1]=r*o,e[2]=i*o,e[3]=a*o,e}function vn(e,t){return e[0]*t[0]+e[1]*t[1]+e[2]*t[2]+e[3]*t[3]}function yn(e,t,n,r){let i=t[0],a=t[1],o=t[2],s=t[3];return e[0]=i+r*(n[0]-i),e[1]=a+r*(n[1]-a),e[2]=o+r*(n[2]-o),e[3]=s+r*(n[3]-s),e}function bn(e,t,n){let r=t[0],i=t[1],a=t[2],o=t[3];return e[0]=n[0]*r+n[4]*i+n[8]*a+n[12]*o,e[1]=n[1]*r+n[5]*i+n[9]*a+n[13]*o,e[2]=n[2]*r+n[6]*i+n[10]*a+n[14]*o,e[3]=n[3]*r+n[7]*i+n[11]*a+n[15]*o,e}function xn(e,t,n){let r=t[0],i=t[1],a=t[2],o=n[0],s=n[1],c=n[2],l=n[3],u=l*r+s*a-c*i,d=l*i+c*r-o*a,f=l*a+o*i-s*r,p=-o*r-s*i-c*a;return e[0]=u*l+p*-o+d*-c-f*-s,e[1]=d*l+p*-s+f*-o-u*-c,e[2]=f*l+p*-c+u*-s-d*-o,e[3]=t[3],e}(function(){let e=fn();return function(t,n,r,i,a,o){let s,c;for(n||=4,r||=0,c=i?Math.min(i*n+r,t.length):t.length,s=r;s<c;s+=n)e[0]=t[s],e[1]=t[s+1],e[2]=t[s+2],e[3]=t[s+3],a(e,e,o),t[s]=e[0],t[s+1]=e[1],t[s+2]=e[2],t[s+3]=e[3];return t}})();var Sn;(function(e){e[e.COL0ROW0=0]=`COL0ROW0`,e[e.COL0ROW1=1]=`COL0ROW1`,e[e.COL0ROW2=2]=`COL0ROW2`,e[e.COL0ROW3=3]=`COL0ROW3`,e[e.COL1ROW0=4]=`COL1ROW0`,e[e.COL1ROW1=5]=`COL1ROW1`,e[e.COL1ROW2=6]=`COL1ROW2`,e[e.COL1ROW3=7]=`COL1ROW3`,e[e.COL2ROW0=8]=`COL2ROW0`,e[e.COL2ROW1=9]=`COL2ROW1`,e[e.COL2ROW2=10]=`COL2ROW2`,e[e.COL2ROW3=11]=`COL2ROW3`,e[e.COL3ROW0=12]=`COL3ROW0`,e[e.COL3ROW1=13]=`COL3ROW1`,e[e.COL3ROW2=14]=`COL3ROW2`,e[e.COL3ROW3=15]=`COL3ROW3`})(Sn||={});var Cn=45*Math.PI/180,wn=1,Tn=.1,En=500,Dn=Object.freeze([1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]),R=class extends At{static get IDENTITY(){return jn()}static get ZERO(){return An()}get ELEMENTS(){return 16}get RANK(){return 4}get INDICES(){return Sn}constructor(e){super(-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0),arguments.length===1&&Array.isArray(e)?this.copy(e):this.identity()}copy(e){return this[0]=e[0],this[1]=e[1],this[2]=e[2],this[3]=e[3],this[4]=e[4],this[5]=e[5],this[6]=e[6],this[7]=e[7],this[8]=e[8],this[9]=e[9],this[10]=e[10],this[11]=e[11],this[12]=e[12],this[13]=e[13],this[14]=e[14],this[15]=e[15],this.check()}set(e,t,n,r,i,a,o,s,c,l,u,d,f,p,m,h){return this[0]=e,this[1]=t,this[2]=n,this[3]=r,this[4]=i,this[5]=a,this[6]=o,this[7]=s,this[8]=c,this[9]=l,this[10]=u,this[11]=d,this[12]=f,this[13]=p,this[14]=m,this[15]=h,this.check()}setRowMajor(e,t,n,r,i,a,o,s,c,l,u,d,f,p,m,h){return this[0]=e,this[1]=i,this[2]=c,this[3]=f,this[4]=t,this[5]=a,this[6]=l,this[7]=p,this[8]=n,this[9]=o,this[10]=u,this[11]=m,this[12]=r,this[13]=s,this[14]=d,this[15]=h,this.check()}toRowMajor(e){return e[0]=this[0],e[1]=this[4],e[2]=this[8],e[3]=this[12],e[4]=this[1],e[5]=this[5],e[6]=this[9],e[7]=this[13],e[8]=this[2],e[9]=this[6],e[10]=this[10],e[11]=this[14],e[12]=this[3],e[13]=this[7],e[14]=this[11],e[15]=this[15],e}identity(){return this.copy(Dn)}fromObject(e){return this.check()}fromQuaternion(e){return an(this,e),this.check()}frustum(e){let{left:t,right:n,bottom:r,top:i,near:a=Tn,far:o=En}=e;return o===1/0?Nn(this,t,n,r,i,a):on(this,t,n,r,i,a,o),this.check()}lookAt(e){let{eye:t,center:n=[0,0,0],up:r=[0,1,0]}=e;return dn(this,t,n,r),this.check()}ortho(e){let{left:t,right:n,bottom:r,top:i,near:a=Tn,far:o=En}=e;return un(this,t,n,r,i,a,o),this.check()}orthographic(e){let{fovy:t=Cn,aspect:n=wn,focalDistance:r=1,near:i=Tn,far:a=En}=e;Mn(t);let o=t/2,s=r*Math.tan(o),c=s*n;return this.ortho({left:-c,right:c,bottom:-s,top:s,near:i,far:a})}perspective(e){let{fovy:t=45*Math.PI/180,aspect:n=1,near:r=.1,far:i=500}=e;return Mn(t),cn(this,t,n,r,i),this.check()}determinant(){return Yt(this)}getScale(e=[-0,-0,-0]){return e[0]=Math.sqrt(this[0]*this[0]+this[1]*this[1]+this[2]*this[2]),e[1]=Math.sqrt(this[4]*this[4]+this[5]*this[5]+this[6]*this[6]),e[2]=Math.sqrt(this[8]*this[8]+this[9]*this[9]+this[10]*this[10]),e}getTranslation(e=[-0,-0,-0]){return e[0]=this[12],e[1]=this[13],e[2]=this[14],e}getRotation(e,t){e||=[-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0,-0],t||=[-0,-0,-0];let n=this.getScale(t),r=1/n[0],i=1/n[1],a=1/n[2];return e[0]=this[0]*r,e[1]=this[1]*i,e[2]=this[2]*a,e[3]=0,e[4]=this[4]*r,e[5]=this[5]*i,e[6]=this[6]*a,e[7]=0,e[8]=this[8]*r,e[9]=this[9]*i,e[10]=this[10]*a,e[11]=0,e[12]=0,e[13]=0,e[14]=0,e[15]=1,e}getRotationMatrix3(e,t){e||=[-0,-0,-0,-0,-0,-0,-0,-0,-0],t||=[-0,-0,-0];let n=this.getScale(t),r=1/n[0],i=1/n[1],a=1/n[2];return e[0]=this[0]*r,e[1]=this[1]*i,e[2]=this[2]*a,e[3]=this[4]*r,e[4]=this[5]*i,e[5]=this[6]*a,e[6]=this[8]*r,e[7]=this[9]*i,e[8]=this[10]*a,e}transpose(){return qt(this,this),this.check()}invert(){return Jt(this,this),this.check()}multiplyLeft(e){return Xt(this,e,this),this.check()}multiplyRight(e){return Xt(this,this,e),this.check()}rotateX(e){return en(this,this,e),this.check()}rotateY(e){return tn(this,this,e),this.check()}rotateZ(e){return nn(this,this,e),this.check()}rotateXYZ(e){return this.rotateX(e[0]).rotateY(e[1]).rotateZ(e[2])}rotateAxis(e,t){return $t(this,this,e,t),this.check()}scale(e){return Qt(this,this,Array.isArray(e)?e:[e,e,e]),this.check()}translate(e){return Zt(this,this,e),this.check()}transform(e,t){return e.length===4?(t=bn(t||[-0,-0,-0,-0],e,this),Be(t,4),t):this.transformAsPoint(e,t)}transformAsPoint(e,t){let{length:n}=e,r;switch(n){case 2:r=Qe(t||[-0,-0],e,this);break;case 3:r=gt(t||[-0,-0,-0],e,this);break;default:throw Error(`Illegal vector`)}return Be(r,e.length),r}transformAsVector(e,t){let n;switch(e.length){case 2:n=et(t||[-0,-0],e,this);break;case 3:n=tt(t||[-0,-0,-0],e,this);break;default:throw Error(`Illegal vector`)}return Be(n,e.length),n}transformPoint(e,t){return this.transformAsPoint(e,t)}transformVector(e,t){return this.transformAsPoint(e,t)}transformDirection(e,t){return this.transformAsVector(e,t)}makeRotationX(e){return this.identity().rotateX(e)}makeTranslation(e,t,n){return this.identity().translate([e,t,n])}},On,kn;function An(){return On||(On=new R([0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]),Object.freeze(On)),On}function jn(){return kn||(kn=new R,Object.freeze(kn)),kn}function Mn(e){if(e>Math.PI*2)throw Error(`expected radians`)}function Nn(e,t,n,r,i,a){let o=2*a/(n-t),s=2*a/(i-r),c=(n+t)/(n-t),l=(i+r)/(i-r),u=-2*a;return e[0]=o,e[1]=0,e[2]=0,e[3]=0,e[4]=0,e[5]=s,e[6]=0,e[7]=0,e[8]=c,e[9]=l,e[10]=-1,e[11]=-1,e[12]=0,e[13]=0,e[14]=u,e[15]=0,e}function Pn(){let e=new Ue(4);return Ue!=Float32Array&&(e[0]=0,e[1]=0,e[2]=0),e[3]=1,e}function Fn(e){return e[0]=0,e[1]=0,e[2]=0,e[3]=1,e}function In(e,t,n){n*=.5;let r=Math.sin(n);return e[0]=r*t[0],e[1]=r*t[1],e[2]=r*t[2],e[3]=Math.cos(n),e}function Ln(e,t,n){let r=t[0],i=t[1],a=t[2],o=t[3],s=n[0],c=n[1],l=n[2],u=n[3];return e[0]=r*u+o*s+i*l-a*c,e[1]=i*u+o*c+a*s-r*l,e[2]=a*u+o*l+r*c-i*s,e[3]=o*u-r*s-i*c-a*l,e}function Rn(e,t,n){n*=.5;let r=t[0],i=t[1],a=t[2],o=t[3],s=Math.sin(n),c=Math.cos(n);return e[0]=r*c+o*s,e[1]=i*c+a*s,e[2]=a*c-i*s,e[3]=o*c-r*s,e}function zn(e,t,n){n*=.5;let r=t[0],i=t[1],a=t[2],o=t[3],s=Math.sin(n),c=Math.cos(n);return e[0]=r*c-a*s,e[1]=i*c+o*s,e[2]=a*c+r*s,e[3]=o*c-i*s,e}function Bn(e,t,n){n*=.5;let r=t[0],i=t[1],a=t[2],o=t[3],s=Math.sin(n),c=Math.cos(n);return e[0]=r*c+i*s,e[1]=i*c-r*s,e[2]=a*c+o*s,e[3]=o*c-a*s,e}function Vn(e,t){let n=t[0],r=t[1],i=t[2];return e[0]=n,e[1]=r,e[2]=i,e[3]=Math.sqrt(Math.abs(1-n*n-r*r-i*i)),e}function Hn(e,t,n,r){let i=t[0],a=t[1],o=t[2],s=t[3],c=n[0],l=n[1],u=n[2],d=n[3],f,p,m,h,g;return f=i*c+a*l+o*u+s*d,f<0&&(f=-f,c=-c,l=-l,u=-u,d=-d),1-f>1e-6?(p=Math.acos(f),g=Math.sin(p),m=Math.sin((1-r)*p)/g,h=Math.sin(r*p)/g):(m=1-r,h=r),e[0]=m*i+h*c,e[1]=m*a+h*l,e[2]=m*o+h*u,e[3]=m*s+h*d,e}function Un(e,t){let n=t[0],r=t[1],i=t[2],a=t[3],o=n*n+r*r+i*i+a*a,s=o?1/o:0;return e[0]=-n*s,e[1]=-r*s,e[2]=-i*s,e[3]=a*s,e}function Wn(e,t){return e[0]=-t[0],e[1]=-t[1],e[2]=-t[2],e[3]=t[3],e}function Gn(e,t){let n=t[0]+t[4]+t[8],r;if(n>0)r=Math.sqrt(n+1),e[3]=.5*r,r=.5/r,e[0]=(t[5]-t[7])*r,e[1]=(t[6]-t[2])*r,e[2]=(t[1]-t[3])*r;else{let n=0;t[4]>t[0]&&(n=1),t[8]>t[n*3+n]&&(n=2);let i=(n+1)%3,a=(n+2)%3;r=Math.sqrt(t[n*3+n]-t[i*3+i]-t[a*3+a]+1),e[n]=.5*r,r=.5/r,e[3]=(t[i*3+a]-t[a*3+i])*r,e[i]=(t[i*3+n]+t[n*3+i])*r,e[a]=(t[a*3+n]+t[n*3+a])*r}return e}var Kn=pn,qn=mn,Jn=vn,Yn=yn,Xn=hn,Zn=gn,Qn=_n,$n=(function(){let e=ot(),t=ct(1,0,0),n=ct(0,1,0);return function(r,i,a){let o=pt(i,a);return o<-.999999?(mt(e,t,i),wt(e)<1e-6&&mt(e,n,i),ft(e,e),In(r,e,Math.PI),r):o>.999999?(r[0]=0,r[1]=0,r[2]=0,r[3]=1,r):(mt(e,i,a),r[0]=e[0],r[1]=e[1],r[2]=e[2],r[3]=1+o,Qn(r,r))}})();(function(){let e=Pn(),t=Pn();return function(n,r,i,a,o,s){return Hn(e,r,o,s),Hn(t,i,a,s),Hn(n,e,t,2*s*(1-s)),n}})(),(function(){let e=jt();return function(t,n,r,i){return e[0]=r[0],e[3]=r[1],e[6]=r[2],e[1]=i[0],e[4]=i[1],e[7]=i[2],e[2]=-n[0],e[5]=-n[1],e[8]=-n[2],Qn(t,Gn(t,e))}})();var er=[0,0,0,1],tr=class extends Re{constructor(e=0,t=0,n=0,r=1){super(-0,-0,-0,-0),Array.isArray(e)&&arguments.length===1?this.copy(e):this.set(e,t,n,r)}copy(e){return this[0]=e[0],this[1]=e[1],this[2]=e[2],this[3]=e[3],this.check()}set(e,t,n,r){return this[0]=e,this[1]=t,this[2]=n,this[3]=r,this.check()}fromObject(e){return this[0]=e.x,this[1]=e.y,this[2]=e.z,this[3]=e.w,this.check()}fromMatrix3(e){return Gn(this,e),this.check()}fromAxisRotation(e,t){return In(this,e,t),this.check()}identity(){return Fn(this),this.check()}setAxisAngle(e,t){return this.fromAxisRotation(e,t)}get ELEMENTS(){return 4}get x(){return this[0]}set x(e){this[0]=F(e)}get y(){return this[1]}set y(e){this[1]=F(e)}get z(){return this[2]}set z(e){this[2]=F(e)}get w(){return this[3]}set w(e){this[3]=F(e)}len(){return Xn(this)}lengthSquared(){return Zn(this)}dot(e){return Jn(this,e)}rotationTo(e,t){return $n(this,e,t),this.check()}add(e){return Kn(this,this,e),this.check()}calculateW(){return Vn(this,this),this.check()}conjugate(){return Wn(this,this),this.check()}invert(){return Un(this,this),this.check()}lerp(e,t,n){return n===void 0?this.lerp(this,e,t):(Yn(this,e,t,n),this.check())}multiplyRight(e){return Ln(this,this,e),this.check()}multiplyLeft(e){return Ln(this,e,this),this.check()}normalize(){let e=this.len(),t=e>0?1/e:0;return this[0]*=t,this[1]*=t,this[2]*=t,this[3]*=t,e===0&&(this[3]=1),this.check()}rotateX(e){return Rn(this,this,e),this.check()}rotateY(e){return zn(this,this,e),this.check()}rotateZ(e){return Bn(this,this,e),this.check()}scale(e){return qn(this,this,e),this.check()}slerp(e,t,n){let r,i,a;switch(arguments.length){case 1:({start:r=er,target:i,ratio:a}=e);break;case 2:r=this,i=e,a=t;break;default:r=e,i=t,a=n}return Hn(this,r,i,a),this.check()}transformVector4(e,t=new kt){return xn(t,e,this),Be(t,4)}lengthSq(){return this.lengthSquared()}setFromAxisAngle(e,t){return this.setAxisAngle(e,t)}premultiply(e){return this.multiplyLeft(e)}multiply(e){return this.multiplyRight(e)}},nr=.1,rr=1e-12,ir=1e-15,ar=1e-20;Math.PI/2,Math.PI/4,Math.PI/6,Math.PI*2;function or(e,t=[],n=0){let r=Math.fround(e),i=e-r;return t[n]=r,t[n+1]=i,t}function sr(e){return e-Math.fround(e)}function cr(e){let t=new Float32Array(32);for(let n=0;n<4;++n)for(let r=0;r<4;++r){let i=n*4+r;or(e[r*4+n],t,i*2)}return t}function lr(e,t=!0){return e??t}function ur(e=[0,0,0],t=!0){return t?e.map(e=>e/255):[...e]}function dr(e,t=!0){let n=ur(e.slice(0,3),t),r=Number.isFinite(e[3]),i=r?e[3]:1;return[n[0],n[1],n[2],t&&r?i/255:i]}var fr=`
layout(std140) uniform fp64arithmeticUniforms {
  uniform float ONE;
  uniform float SPLIT;
} fp64;

/*
About LUMA_FP64_CODE_ELIMINATION_WORKAROUND

The purpose of this workaround is to prevent shader compilers from
optimizing away necessary arithmetic operations by swapping their sequences
or transform the equation to some 'equivalent' form.

These helpers implement Dekker/Veltkamp-style error tracking. If the compiler
folds constants or reassociates the arithmetic, the high/low split can stop
tracking the rounding error correctly. That failure mode tends to look fine in
simple coordinate setup, but then breaks down inside iterative arithmetic such
as fp64 Mandelbrot loops.

The method is to multiply an artifical variable, ONE, which will be known to
the compiler to be 1 only at runtime. The whole expression is then represented
as a polynomial with respective to ONE. In the coefficients of all terms, only one a
and one b should appear

err = (a + b) * ONE^6 - a * ONE^5 - (a + b) * ONE^4 + a * ONE^3 - b - (a + b) * ONE^2 + a * ONE
*/

float prevent_fp64_optimization(float value) {
#if defined(LUMA_FP64_CODE_ELIMINATION_WORKAROUND)
  return value + fp64.ONE * 0.0;
#else
  return value;
#endif
}

// Divide float number to high and low floats to extend fraction bits
vec2 split(float a) {
  // Keep SPLIT as a runtime uniform so the compiler cannot fold the Dekker
  // split into a constant expression and reassociate the recovery steps.
  float split = prevent_fp64_optimization(fp64.SPLIT);
  float t = prevent_fp64_optimization(a * split);
  float temp = t - a;
  float a_hi = t - temp;
  float a_lo = a - a_hi;
  return vec2(a_hi, a_lo);
}

// Divide float number again when high float uses too many fraction bits
vec2 split2(vec2 a) {
  vec2 b = split(a.x);
  b.y += a.y;
  return b;
}

// Special sum operation when a > b
vec2 quickTwoSum(float a, float b) {
#if defined(LUMA_FP64_CODE_ELIMINATION_WORKAROUND)
  float sum = (a + b) * fp64.ONE;
  float err = b - (sum - a) * fp64.ONE;
#else
  float sum = a + b;
  float err = b - (sum - a);
#endif
  return vec2(sum, err);
}

// General sum operation
vec2 twoSum(float a, float b) {
  float s = (a + b);
#if defined(LUMA_FP64_CODE_ELIMINATION_WORKAROUND)
  float v = (s * fp64.ONE - a) * fp64.ONE;
  float err = (a - (s - v) * fp64.ONE) * fp64.ONE * fp64.ONE * fp64.ONE + (b - v);
#else
  float v = s - a;
  float err = (a - (s - v)) + (b - v);
#endif
  return vec2(s, err);
}

vec2 twoSub(float a, float b) {
  float s = (a - b);
#if defined(LUMA_FP64_CODE_ELIMINATION_WORKAROUND)
  float v = (s * fp64.ONE - a) * fp64.ONE;
  float err = (a - (s - v) * fp64.ONE) * fp64.ONE * fp64.ONE * fp64.ONE - (b + v);
#else
  float v = s - a;
  float err = (a - (s - v)) - (b + v);
#endif
  return vec2(s, err);
}

vec2 twoSqr(float a) {
  float prod = a * a;
  vec2 a_fp64 = split(a);
#if defined(LUMA_FP64_CODE_ELIMINATION_WORKAROUND)
  float err = ((a_fp64.x * a_fp64.x - prod) * fp64.ONE + 2.0 * a_fp64.x *
    a_fp64.y * fp64.ONE * fp64.ONE) + a_fp64.y * a_fp64.y * fp64.ONE * fp64.ONE * fp64.ONE;
#else
  float err = ((a_fp64.x * a_fp64.x - prod) + 2.0 * a_fp64.x * a_fp64.y) + a_fp64.y * a_fp64.y;
#endif
  return vec2(prod, err);
}

vec2 twoProd(float a, float b) {
  float prod = a * b;
  vec2 a_fp64 = split(a);
  vec2 b_fp64 = split(b);
  // twoProd is especially sensitive because mul_fp64 and div_fp64 both depend
  // on the split terms and cross terms staying in the original evaluation
  // order. If the compiler folds or reassociates them, the low part tends to
  // collapse to zero or NaN on some drivers.
  float highProduct = prevent_fp64_optimization(a_fp64.x * b_fp64.x);
  float crossProduct1 = prevent_fp64_optimization(a_fp64.x * b_fp64.y);
  float crossProduct2 = prevent_fp64_optimization(a_fp64.y * b_fp64.x);
  float lowProduct = prevent_fp64_optimization(a_fp64.y * b_fp64.y);
#if defined(LUMA_FP64_CODE_ELIMINATION_WORKAROUND)
  float err1 = (highProduct - prod) * fp64.ONE;
  float err2 = crossProduct1 * fp64.ONE * fp64.ONE;
  float err3 = crossProduct2 * fp64.ONE * fp64.ONE * fp64.ONE;
  float err4 = lowProduct * fp64.ONE * fp64.ONE * fp64.ONE * fp64.ONE;
#else
  float err1 = highProduct - prod;
  float err2 = crossProduct1;
  float err3 = crossProduct2;
  float err4 = lowProduct;
#endif
  float err = ((err1 + err2) + err3) + err4;
  return vec2(prod, err);
}

vec2 sum_fp64(vec2 a, vec2 b) {
  vec2 s, t;
  s = twoSum(a.x, b.x);
  t = twoSum(a.y, b.y);
  s.y += t.x;
  s = quickTwoSum(s.x, s.y);
  s.y += t.y;
  s = quickTwoSum(s.x, s.y);
  return s;
}

vec2 sub_fp64(vec2 a, vec2 b) {
  vec2 s, t;
  s = twoSub(a.x, b.x);
  t = twoSub(a.y, b.y);
  s.y += t.x;
  s = quickTwoSum(s.x, s.y);
  s.y += t.y;
  s = quickTwoSum(s.x, s.y);
  return s;
}

vec2 mul_fp64(vec2 a, vec2 b) {
  vec2 prod = twoProd(a.x, b.x);
  // y component is for the error
  prod.y += a.x * b.y;
#if defined(LUMA_FP64_HIGH_BITS_OVERFLOW_WORKAROUND)
  prod = split2(prod);
#endif
  prod = quickTwoSum(prod.x, prod.y);
  prod.y += a.y * b.x;
#if defined(LUMA_FP64_HIGH_BITS_OVERFLOW_WORKAROUND)
  prod = split2(prod);
#endif
  prod = quickTwoSum(prod.x, prod.y);
  return prod;
}

vec2 div_fp64(vec2 a, vec2 b) {
  float xn = 1.0 / b.x;
#if defined(LUMA_FP64_HIGH_BITS_OVERFLOW_WORKAROUND)
  vec2 yn = mul_fp64(a, vec2(xn, 0));
#else
  vec2 yn = a * xn;
#endif
  float diff = (sub_fp64(a, mul_fp64(b, yn))).x;
  vec2 prod = twoProd(xn, diff);
  return sum_fp64(yn, prod);
}

vec2 sqrt_fp64(vec2 a) {
  if (a.x == 0.0 && a.y == 0.0) return vec2(0.0, 0.0);
  if (a.x < 0.0) return vec2(0.0 / 0.0, 0.0 / 0.0);

  float x = 1.0 / sqrt(a.x);
  float yn = a.x * x;
#if defined(LUMA_FP64_CODE_ELIMINATION_WORKAROUND)
  vec2 yn_sqr = twoSqr(yn) * fp64.ONE;
#else
  vec2 yn_sqr = twoSqr(yn);
#endif
  float diff = sub_fp64(a, yn_sqr).x;
  vec2 prod = twoProd(x * 0.5, diff);
#if defined(LUMA_FP64_HIGH_BITS_OVERFLOW_WORKAROUND)
  return sum_fp64(split(yn), prod);
#else
  return sum_fp64(vec2(yn, 0.0), prod);
#endif
}
`,pr={name:`fp64arithmetic`,source:`struct Fp64ArithmeticUniforms {
  ONE: f32,
  SPLIT: f32,
};

@group(0) @binding(auto) var<uniform> fp64arithmetic : Fp64ArithmeticUniforms;

#ifndef LUMA_FP64_F32_INPUT_ONLY
struct Fp64Bits {
  sign: u32,
  exponent: i32,
  significand: vec2u,
  isZero: bool,
  isInf: bool,
  isNan: bool,
};
#endif

#ifndef LUMA_FP64_PREDICATE_ONLY
fn fp64_nan(seed: f32) -> f32 {
  let nanBits = 0x7fc00000u | select(0u, 1u, seed < 0.0);
  return bitcast<f32>(nanBits);
}
#endif

fn fp64_u64_is_zero(value: vec2u) -> bool {
  return value.x == 0u && value.y == 0u;
}

fn fp64_u64_compare(a: vec2u, b: vec2u) -> i32 {
  if (a.x != b.x) {
    return select(-1, 1, a.x > b.x);
  }
  if (a.y != b.y) {
    return select(-1, 1, a.y > b.y);
  }
  return 0;
}

fn fp64_u64_add(a: vec2u, b: vec2u) -> vec2u {
  let low = a.y + b.y;
  let carry = select(0u, 1u, low < a.y);
  return vec2u(a.x + b.x + carry, low);
}

fn fp64_u64_sub(a: vec2u, b: vec2u) -> vec2u {
  let borrow = select(0u, 1u, a.y < b.y);
  return vec2u(a.x - b.x - borrow, a.y - b.y);
}

fn fp64_u64_shift_left(value: vec2u, shift: u32) -> vec2u {
  if (shift == 0u) {
    return value;
  }
  if (shift < 32u) {
    return vec2u((value.x << shift) | (value.y >> (32u - shift)), value.y << shift);
  }
  if (shift == 32u) {
    return vec2u(value.y, 0u);
  }
  if (shift < 64u) {
    return vec2u(value.y << (shift - 32u), 0u);
  }
  return vec2u(0u);
}

fn fp64_u64_shift_right(value: vec2u, shift: u32) -> vec2u {
  if (shift == 0u) {
    return value;
  }
  if (shift < 32u) {
    return vec2u(value.x >> shift, (value.y >> shift) | (value.x << (32u - shift)));
  }
  if (shift == 32u) {
    return vec2u(0u, value.x);
  }
  if (shift < 64u) {
    return vec2u(0u, value.x >> (shift - 32u));
  }
  return vec2u(0u);
}

fn fp64_u64_get_bit(value: vec2u, bitIndex: u32) -> bool {
  if (bitIndex >= 64u) {
    return false;
  }
  if (bitIndex >= 32u) {
    return ((value.x >> (bitIndex - 32u)) & 1u) != 0u;
  }
  return ((value.y >> bitIndex) & 1u) != 0u;
}

fn fp64_u64_has_bits_below(value: vec2u, bitCount: u32) -> bool {
  if (bitCount == 0u) {
    return false;
  }
  if (bitCount >= 64u) {
    return !fp64_u64_is_zero(value);
  }
  if (bitCount > 32u) {
    let highBitCount = bitCount - 32u;
    let highMask = (1u << highBitCount) - 1u;
    return value.y != 0u || (value.x & highMask) != 0u;
  }
  if (bitCount == 32u) {
    return value.y != 0u;
  }
  let lowMask = (1u << bitCount) - 1u;
  return (value.y & lowMask) != 0u;
}

#ifndef LUMA_FP64_F32_INPUT_ONLY
fn fp64_u64_shift_right_sticky(value: vec2u, shift: u32) -> vec2u {
  var shifted = fp64_u64_shift_right(value, shift);
  if (fp64_u64_has_bits_below(value, shift)) {
    shifted.y = shifted.y | 1u;
  }
  return shifted;
}
#endif

fn fp64_u64_count_leading_zeros(value: vec2u) -> u32 {
  if (value.x != 0u) {
    return countLeadingZeros(value.x);
  }
  return 32u + countLeadingZeros(value.y);
}

fn fp64_round_shift_right_to_u32(value: vec2u, shift: u32) -> u32 {
  if (shift == 0u) {
    return value.y;
  }

  let truncated = fp64_u64_shift_right(value, shift);
  var rounded = truncated.y;
  let guard = fp64_u64_get_bit(value, shift - 1u);
  let hasTrailingBits = fp64_u64_has_bits_below(value, shift - 1u);
  if (guard && (hasTrailingBits || (rounded & 1u) == 1u)) {
    rounded = rounded + 1u;
  }
  return rounded;
}

#ifndef LUMA_FP64_F32_INPUT_ONLY
fn fp64_round_shift_right(value: vec2u, shift: u32) -> vec2u {
  if (shift == 0u) {
    return value;
  }

  var rounded = fp64_u64_shift_right(value, shift);
  let guard = fp64_u64_get_bit(value, shift - 1u);
  let hasTrailingBits = fp64_u64_has_bits_below(value, shift - 1u);
  if (guard && (hasTrailingBits || (rounded.y & 1u) == 1u)) {
    rounded = fp64_u64_add(rounded, vec2u(0u, 1u));
  }
  return rounded;
}
#endif

fn fp64_make_f32_bits_from_u64(sign: u32, significand: vec2u, baseExponent: i32) -> u32 {
  if (fp64_u64_is_zero(significand)) {
    return sign << 31u;
  }

  let leadingZeros = fp64_u64_count_leading_zeros(significand);
  let mostSignificantBit = 63u - leadingZeros;
  var exponent = baseExponent + i32(mostSignificantBit);

  if (exponent > 127) {
    return (sign << 31u) | 0x7f800000u;
  }

  if (exponent >= -126) {
    let shift = i32(mostSignificantBit) - 23;
    var significand24: u32;
    if (shift > 0) {
      significand24 = fp64_round_shift_right_to_u32(significand, u32(shift));
    } else {
      significand24 = fp64_u64_shift_left(significand, u32(-shift)).y;
    }

    if (significand24 >= 0x1000000u) {
      significand24 = significand24 >> 1u;
      exponent = exponent + 1;
      if (exponent > 127) {
        return (sign << 31u) | 0x7f800000u;
      }
    }

    return (sign << 31u) | (u32(exponent + 127) << 23u) | (significand24 & 0x7fffffu);
  }

  let scaleExponent = baseExponent + 149;
  var mantissa: u32;
  if (scaleExponent >= 0) {
    mantissa = fp64_u64_shift_left(significand, u32(scaleExponent)).y;
  } else {
    mantissa = fp64_round_shift_right_to_u32(significand, u32(-scaleExponent));
  }

  if (mantissa >= 0x800000u) {
    return (sign << 31u) | 0x00800000u;
  }
  return (sign << 31u) | mantissa;
}

#ifndef LUMA_FP64_F32_INPUT_ONLY
fn fp64_decode_bits(bits: vec2u) -> Fp64Bits {
  let sign = bits.x >> 31u;
  let exponentBits = (bits.x >> 20u) & 0x7ffu;
  let fractionHigh = bits.x & 0xfffffu;
  let fractionLow = bits.y;
  let fraction = vec2u(fractionHigh, fractionLow);

  if (exponentBits == 0x7ffu) {
    let isInf = fp64_u64_is_zero(fraction);
    return Fp64Bits(sign, 0, vec2u(0u), false, isInf, !isInf);
  }

  if (exponentBits == 0u) {
    let isZero = fp64_u64_is_zero(fraction);
    return Fp64Bits(sign, -1022, fraction, isZero, false, false);
  }

  return Fp64Bits(sign, i32(exponentBits) - 1023, vec2u((1u << 20u) | fractionHigh, fractionLow), false, false, false);
}

fn fp64_finite_magnitude_compare(a: Fp64Bits, b: Fp64Bits) -> i32 {
  if (a.exponent != b.exponent) {
    return select(-1, 1, a.exponent > b.exponent);
  }
  return fp64_u64_compare(a.significand, b.significand);
}
#endif

#ifndef LUMA_FP64_F32_INPUT_ONLY
struct Fp64RawF32Bits {
  sign: u32,
  baseExponent: i32,
  significand: u32,
  isZero: bool,
  isInf: bool,
  isNan: bool,
};

// Decode an f32 as (-1)^sign * significand * 2^baseExponent. This shared
// integer representation lets normalization remain independent of the
// selected double-single arithmetic implementation.
fn fp64_decode_raw_f32_bits(bits: u32) -> Fp64RawF32Bits {
  let sign = bits >> 31u;
  let exponentBits = (bits >> 23u) & 0xffu;
  let fraction = bits & 0x7fffffu;

  if (exponentBits == 0xffu) {
    return Fp64RawF32Bits(sign, 0, 0u, false, fraction == 0u, fraction != 0u);
  }
  if (exponentBits == 0u) {
    return Fp64RawF32Bits(sign, -149, fraction, fraction == 0u, false, false);
  }
  return Fp64RawF32Bits(
    sign,
    i32(exponentBits) - 150,
    0x800000u | fraction,
    false,
    false,
    false
  );
}

fn fp64_raw_f32_magnitude_compare(aBits: u32, bBits: u32) -> i32 {
  let aMagnitude = aBits & 0x7fffffffu;
  let bMagnitude = bBits & 0x7fffffffu;
  if (aMagnitude == bMagnitude) {
    return 0;
  }
  return select(-1, 1, aMagnitude > bMagnitude);
}

fn fp64_make_raw_residual_f32_bits(
  exactSign: u32,
  exactMagnitude: vec2u,
  exactBaseExponent: i32,
  highBits: u32
) -> u32 {
  if (fp64_u64_is_zero(exactMagnitude)) {
    return 0u;
  }

  let high = fp64_decode_raw_f32_bits(highBits);
  if (high.isInf || high.isNan) {
    return 0u;
  }
  if (high.isZero) {
    return fp64_make_f32_bits_from_u64(exactSign, exactMagnitude, exactBaseExponent);
  }

  let commonBaseExponent = min(exactBaseExponent, high.baseExponent);
  let exactShift = exactBaseExponent - commonBaseExponent;
  let highShift = high.baseExponent - commonBaseExponent;
  if (exactShift >= 64 || highShift >= 64) {
    return 0u;
  }

  let exactAligned = fp64_u64_shift_left(exactMagnitude, u32(exactShift));
  let highAligned = fp64_u64_shift_left(vec2u(0u, high.significand), u32(highShift));
  let comparison = fp64_u64_compare(exactAligned, highAligned);
  if (comparison == 0) {
    return 0u;
  }

  var residualSign = exactSign;
  var residualMagnitude: vec2u;
  if (comparison > 0) {
    residualMagnitude = fp64_u64_sub(exactAligned, highAligned);
  } else {
    residualSign = exactSign ^ 1u;
    residualMagnitude = fp64_u64_sub(highAligned, exactAligned);
  }
  return fp64_make_f32_bits_from_u64(
    residualSign,
    residualMagnitude,
    commonBaseExponent
  );
}

fn fp64_split_raw_accumulator_bits(
  sign: u32,
  magnitude: vec2u,
  baseExponent: i32
) -> vec2u {
  if (fp64_u64_is_zero(magnitude)) {
    return vec2u(0u);
  }
  let highBits = fp64_make_f32_bits_from_u64(sign, magnitude, baseExponent);
  let rawLowBits = fp64_make_raw_residual_f32_bits(sign, magnitude, baseExponent, highBits);
  let lowBits = select(rawLowBits, 0u, (rawLowBits & 0x7fffffffu) == 0u);
  if ((highBits & 0x7fffffffu) == 0u && (lowBits & 0x7fffffffu) == 0u) {
    return vec2u(0u);
  }
  return vec2u(highBits, lowBits);
}
#endif

#ifndef LUMA_FP64_F32_INPUT_ONLY
// Round an arithmetic accumulator to binary64 before splitting it. The
// aligned add/subtract paths retain three guard bits plus a sticky bit, which
// is sufficient for round-to-nearest-even at the binary64 boundary.
fn fp64_split_binary64_accumulator_bits(
  sign: u32,
  magnitude: vec2u,
  baseExponent: i32
) -> vec2u {
  if (fp64_u64_is_zero(magnitude)) {
    return vec2u(0u);
  }

  let mostSignificantBit = 63u - fp64_u64_count_leading_zeros(magnitude);
  let exponent = baseExponent + i32(mostSignificantBit);
  if (exponent > 1023) {
    return vec2u((sign << 31u) | 0x7f800000u, 0u);
  }

  var roundedMagnitude = magnitude;
  var roundedBaseExponent = baseExponent;
  if (exponent >= -1022) {
    if (mostSignificantBit > 52u) {
      let shift = mostSignificantBit - 52u;
      roundedMagnitude = fp64_round_shift_right(magnitude, shift);
      roundedBaseExponent = baseExponent + i32(shift);
    }
  } else {
    let shift = -1074 - baseExponent;
    if (shift > 0) {
      roundedMagnitude = fp64_round_shift_right(magnitude, u32(shift));
      roundedBaseExponent = -1074;
    }
  }

  if (fp64_u64_is_zero(roundedMagnitude)) {
    return vec2u(0u);
  }
  return fp64_split_raw_accumulator_bits(sign, roundedMagnitude, roundedBaseExponent);
}
#endif

#ifndef LUMA_FP64_PREDICATE_ONLY
fn fp64_add_raw_f32_bits(aBits: u32, bBits: u32) -> vec2u {
  let a = fp64_decode_raw_f32_bits(aBits);
  let b = fp64_decode_raw_f32_bits(bBits);

  if (a.isNan || b.isNan) {
    return vec2u(0x7fc00000u, 0u);
  }
  if (a.isInf || b.isInf) {
    if (a.isInf && b.isInf && a.sign != b.sign) {
      return vec2u(0x7fc00000u, 0u);
    }
    return select(vec2u(bBits, 0u), vec2u(aBits, 0u), a.isInf);
  }
  if (a.isZero && b.isZero) {
    return vec2u(0u);
  }
  if (a.isZero) {
    return vec2u(bBits, 0u);
  }
  if (b.isZero) {
    return vec2u(aBits, 0u);
  }

  let exponentDifference = abs(a.baseExponent - b.baseExponent);
  if (exponentDifference > 25) {
    if (fp64_raw_f32_magnitude_compare(aBits, bBits) >= 0) {
      return vec2u(aBits, bBits);
    }
    return vec2u(bBits, aBits);
  }

  let commonBaseExponent = min(a.baseExponent, b.baseExponent);
  let aMagnitude = fp64_u64_shift_left(
    vec2u(0u, a.significand),
    u32(a.baseExponent - commonBaseExponent)
  );
  let bMagnitude = fp64_u64_shift_left(
    vec2u(0u, b.significand),
    u32(b.baseExponent - commonBaseExponent)
  );

  var resultSign = a.sign;
  var resultMagnitude: vec2u;
  if (a.sign == b.sign) {
    resultMagnitude = fp64_u64_add(aMagnitude, bMagnitude);
  } else {
    let comparison = fp64_u64_compare(aMagnitude, bMagnitude);
    if (comparison == 0) {
      return vec2u(0u);
    }
    if (comparison > 0) {
      resultMagnitude = fp64_u64_sub(aMagnitude, bMagnitude);
    } else {
      resultSign = b.sign;
      resultMagnitude = fp64_u64_sub(bMagnitude, aMagnitude);
    }
  }

  return fp64_split_raw_accumulator_bits(
    resultSign,
    resultMagnitude,
    commonBaseExponent
  );
}
#endif

#ifndef LUMA_FP64_F32_INPUT_ONLY
fn fp64_add_aligned_magnitudes_to_fp64_bits(
  sign: u32,
  larger: Fp64Bits,
  smaller: Fp64Bits
) -> vec2u {
  let largeSignificand = fp64_u64_shift_left(larger.significand, 3u);
  let smallSignificand = fp64_u64_shift_right_sticky(
    fp64_u64_shift_left(smaller.significand, 3u),
    u32(larger.exponent - smaller.exponent)
  );
  let resultSignificand = fp64_u64_add(largeSignificand, smallSignificand);
  return fp64_split_binary64_accumulator_bits(
    sign,
    resultSignificand,
    larger.exponent - 55
  );
}

fn fp64_sub_aligned_magnitudes_to_fp64_bits(
  sign: u32,
  larger: Fp64Bits,
  smaller: Fp64Bits
) -> vec2u {
  let largeSignificand = fp64_u64_shift_left(larger.significand, 3u);
  let smallSignificand = fp64_u64_shift_right_sticky(
    fp64_u64_shift_left(smaller.significand, 3u),
    u32(larger.exponent - smaller.exponent)
  );
  let resultSignificand = fp64_u64_sub(largeSignificand, smallSignificand);
  return fp64_split_binary64_accumulator_bits(
    sign,
    resultSignificand,
    larger.exponent - 55
  );
}

fn fp64_add_aligned_magnitudes_to_f32_bits(sign: u32, larger: Fp64Bits, smaller: Fp64Bits) -> u32 {
  let largeSignificand = fp64_u64_shift_left(larger.significand, 3u);
  let smallSignificand = fp64_u64_shift_right_sticky(
    fp64_u64_shift_left(smaller.significand, 3u),
    u32(larger.exponent - smaller.exponent)
  );
  let resultSignificand = fp64_u64_add(largeSignificand, smallSignificand);
  return fp64_make_f32_bits_from_u64(sign, resultSignificand, larger.exponent - 55);
}

fn fp64_sub_aligned_magnitudes_to_f32_bits(sign: u32, larger: Fp64Bits, smaller: Fp64Bits) -> u32 {
  let largeSignificand = fp64_u64_shift_left(larger.significand, 3u);
  let smallSignificand = fp64_u64_shift_right_sticky(
    fp64_u64_shift_left(smaller.significand, 3u),
    u32(larger.exponent - smaller.exponent)
  );
  let resultSignificand = fp64_u64_sub(largeSignificand, smallSignificand);
  return fp64_make_f32_bits_from_u64(sign, resultSignificand, larger.exponent - 55);
}

// Subtract two raw binary64 values and round the exact result once to f32.
// The input words are canonical high/low words: .x contains sign/exponent/high
// fraction bits, and .y contains the low 32 fraction bits.
fn sub_fp64u32_to_f32_bits(aBits: vec2u, bBits: vec2u) -> u32 {
  let a = fp64_decode_bits(aBits);
  let b = fp64_decode_bits(bBits);
  let bSubtractionSign = b.sign ^ 1u;

  if (a.isNan || b.isNan) {
    return 0x7fc00000u;
  }
  if (a.isInf && b.isInf) {
    if (a.sign == bSubtractionSign) {
      return (a.sign << 31u) | 0x7f800000u;
    }
    return 0x7fc00000u;
  }
  if (a.isInf) {
    return (a.sign << 31u) | 0x7f800000u;
  }
  if (b.isInf) {
    return (bSubtractionSign << 31u) | 0x7f800000u;
  }
  if (a.isZero && b.isZero) {
    return select(0u, 0x80000000u, a.sign == 1u && b.sign == 0u);
  }

  let magnitudeComparison = fp64_finite_magnitude_compare(a, b);
  if (a.sign == bSubtractionSign) {
    if (magnitudeComparison >= 0) {
      return fp64_add_aligned_magnitudes_to_f32_bits(a.sign, a, b);
    }
    return fp64_add_aligned_magnitudes_to_f32_bits(a.sign, b, a);
  }

  if (magnitudeComparison == 0) {
    return 0u;
  }
  if (magnitudeComparison > 0) {
    return fp64_sub_aligned_magnitudes_to_f32_bits(a.sign, a, b);
  }
  return fp64_sub_aligned_magnitudes_to_f32_bits(bSubtractionSign, b, a);
}

fn sub_fp64u32_to_f32(aBits: vec2u, bBits: vec2u) -> f32 {
  return bitcast<f32>(sub_fp64u32_to_f32_bits(aBits, bBits));
}

// Subtract two raw binary64 values, round once to binary64, then split the
// result into normalized f32 limbs. Finite results must fit within the f32
// exponent range; larger magnitudes map to infinity and smaller magnitudes
// map to zero. The input words use canonical high/low word order.
fn sub_fp64u32_to_fp64_bits(aBits: vec2u, bBits: vec2u) -> vec2u {
  let a = fp64_decode_bits(aBits);
  let b = fp64_decode_bits(bBits);
  let bSubtractionSign = b.sign ^ 1u;

  if (a.isNan || b.isNan) {
    return vec2u(0x7fc00000u, 0u);
  }
  if (a.isInf && b.isInf) {
    if (a.sign == bSubtractionSign) {
      return vec2u((a.sign << 31u) | 0x7f800000u, 0u);
    }
    return vec2u(0x7fc00000u, 0u);
  }
  if (a.isInf) {
    return vec2u((a.sign << 31u) | 0x7f800000u, 0u);
  }
  if (b.isInf) {
    return vec2u((bSubtractionSign << 31u) | 0x7f800000u, 0u);
  }
  if (a.isZero && b.isZero) {
    return vec2u(0u);
  }

  let magnitudeComparison = fp64_finite_magnitude_compare(a, b);
  if (a.sign == bSubtractionSign) {
    if (magnitudeComparison >= 0) {
      return fp64_add_aligned_magnitudes_to_fp64_bits(a.sign, a, b);
    }
    return fp64_add_aligned_magnitudes_to_fp64_bits(a.sign, b, a);
  }

  if (magnitudeComparison == 0) {
    return vec2u(0u);
  }
  if (magnitudeComparison > 0) {
    return fp64_sub_aligned_magnitudes_to_fp64_bits(a.sign, a, b);
  }
  return fp64_sub_aligned_magnitudes_to_fp64_bits(bSubtractionSign, b, a);
}

fn sub_fp64u32_to_fp64(aBits: vec2u, bBits: vec2u) -> vec2f {
  let resultBits = sub_fp64u32_to_fp64_bits(aBits, bBits);
  return vec2f(bitcast<f32>(resultBits.x), bitcast<f32>(resultBits.y));
}
#endif

#ifndef LUMA_FP64_PREDICATE_ONLY
fn fp64_runtime_zero() -> f32 {
  return fp64arithmetic.ONE * 0.0;
}

fn prevent_fp64_optimization(value: f32) -> f32 {
#ifdef LUMA_FP64_CODE_ELIMINATION_WORKAROUND
  return value + fp64_runtime_zero();
#else
  return value;
#endif
}
#endif

#ifdef LUMA_FP64_INTEGER_ARITHMETIC
struct Fp64F32Bits {
  sign: u32,
  baseExponent: i32,
  significand: u32,
  isZero: bool,
  isInf: bool,
  isNan: bool,
};

// Decode an f32 as (-1)^sign * significand * 2^baseExponent.
fn fp64_decode_f32_bits(bits: u32) -> Fp64F32Bits {
  let sign = bits >> 31u;
  let exponentBits = (bits >> 23u) & 0xffu;
  let fraction = bits & 0x7fffffu;

  if (exponentBits == 0xffu) {
    return Fp64F32Bits(sign, 0, 0u, false, fraction == 0u, fraction != 0u);
  }
  if (exponentBits == 0u) {
    return Fp64F32Bits(sign, -149, fraction, fraction == 0u, false, false);
  }
  return Fp64F32Bits(sign, i32(exponentBits) - 150, 0x800000u | fraction, false, false, false);
}

fn fp64_f32_magnitude_compare(aBits: u32, bBits: u32) -> i32 {
  let aMagnitude = aBits & 0x7fffffffu;
  let bMagnitude = bBits & 0x7fffffffu;
  if (aMagnitude == bMagnitude) {
    return 0;
  }
  return select(-1, 1, aMagnitude > bMagnitude);
}

fn fp64_make_residual_f32_bits(
  exactSign: u32,
  exactMagnitude: vec2u,
  exactBaseExponent: i32,
  highBits: u32
) -> u32 {
  if (fp64_u64_is_zero(exactMagnitude)) {
    return 0u;
  }

  let high = fp64_decode_f32_bits(highBits);
  if (high.isInf || high.isNan) {
    return exactSign << 31u;
  }
  if (high.isZero) {
    return fp64_make_f32_bits_from_u64(exactSign, exactMagnitude, exactBaseExponent);
  }

  let commonBaseExponent = min(exactBaseExponent, high.baseExponent);
  let exactShift = exactBaseExponent - commonBaseExponent;
  let highShift = high.baseExponent - commonBaseExponent;

  // A normal two-sum/two-product residual never needs a shift this large.
  // This guard gives deterministic underflow behavior outside that contract.
  if (exactShift >= 64 || highShift >= 64) {
    return exactSign << 31u;
  }

  let exactAligned = fp64_u64_shift_left(exactMagnitude, u32(exactShift));
  let highAligned = fp64_u64_shift_left(vec2u(0u, high.significand), u32(highShift));
  let comparison = fp64_u64_compare(exactAligned, highAligned);
  if (comparison == 0) {
    return 0u;
  }

  var residualSign = exactSign;
  var residualMagnitude: vec2u;
  if (comparison > 0) {
    residualMagnitude = fp64_u64_sub(exactAligned, highAligned);
  } else {
    residualSign = exactSign ^ 1u;
    residualMagnitude = fp64_u64_sub(highAligned, exactAligned);
  }
  return fp64_make_f32_bits_from_u64(
    residualSign,
    residualMagnitude,
    commonBaseExponent
  );
}

fn fp64_split_accumulator_bits(
  sign: u32,
  magnitude: vec2u,
  baseExponent: i32
) -> vec2u {
  let highBits = fp64_make_f32_bits_from_u64(sign, magnitude, baseExponent);
  let lowBits = fp64_make_residual_f32_bits(sign, magnitude, baseExponent, highBits);
  return vec2u(highBits, lowBits);
}

fn fp64_two_sum_integer_bits(aBits: u32, bBits: u32) -> vec2u {
  let a = fp64_decode_f32_bits(aBits);
  let b = fp64_decode_f32_bits(bBits);

  if (a.isNan || b.isNan) {
    return vec2u(0x7fc00000u, 0u);
  }
  if (a.isInf || b.isInf) {
    if (a.isInf && b.isInf && a.sign != b.sign) {
      return vec2u(0x7fc00000u, 0u);
    }
    return select(vec2u(bBits, 0u), vec2u(aBits, 0u), a.isInf);
  }
  if (a.isZero && b.isZero) {
    return vec2u((a.sign & b.sign) << 31u, 0u);
  }
  if (a.isZero) {
    return vec2u(bBits, 0u);
  }
  if (b.isZero) {
    return vec2u(aBits, 0u);
  }

  let exponentDifference = select(
    b.baseExponent - a.baseExponent,
    a.baseExponent - b.baseExponent,
    a.baseExponent >= b.baseExponent
  );

  // Beyond half an ulp, rounding cannot change the larger operand. Returning
  // the smaller operand intact also avoids an unbounded integer alignment.
  // At a power-of-two boundary the spacing below the larger operand is half
  // the spacing above it, so an opposite-sign gap-25 operand can still change
  // the rounded high limb. Gap 26 is the first universally safe early-out.
  if (exponentDifference > 25) {
    if (fp64_f32_magnitude_compare(aBits, bBits) >= 0) {
      return vec2u(aBits, bBits);
    }
    return vec2u(bBits, aBits);
  }

  let commonBaseExponent = min(a.baseExponent, b.baseExponent);
  let aMagnitude = fp64_u64_shift_left(
    vec2u(0u, a.significand),
    u32(a.baseExponent - commonBaseExponent)
  );
  let bMagnitude = fp64_u64_shift_left(
    vec2u(0u, b.significand),
    u32(b.baseExponent - commonBaseExponent)
  );

  var resultSign = a.sign;
  var resultMagnitude: vec2u;
  if (a.sign == b.sign) {
    resultMagnitude = fp64_u64_add(aMagnitude, bMagnitude);
  } else {
    let comparison = fp64_u64_compare(aMagnitude, bMagnitude);
    if (comparison == 0) {
      return vec2u(0u, 0u);
    }
    if (comparison > 0) {
      resultMagnitude = fp64_u64_sub(aMagnitude, bMagnitude);
    } else {
      resultSign = b.sign;
      resultMagnitude = fp64_u64_sub(bMagnitude, aMagnitude);
    }
  }

  return fp64_split_accumulator_bits(resultSign, resultMagnitude, commonBaseExponent);
}

fn fp64_two_sum_integer(a: f32, b: f32) -> vec2f {
  let resultBits = fp64_two_sum_integer_bits(bitcast<u32>(a), bitcast<u32>(b));
  return vec2f(bitcast<f32>(resultBits.x), bitcast<f32>(resultBits.y));
}

fn fp64_multiply_significands(a: u32, b: u32) -> vec2u {
  let aLow = a & 0xffffu;
  let aHigh = a >> 16u;
  let bLow = b & 0xffffu;
  let bHigh = b >> 16u;
  let lowProduct = aLow * bLow;
  let crossProduct = aLow * bHigh + aHigh * bLow;
  let highProduct = aHigh * bHigh;

  var result = vec2u(0u, lowProduct);
  result = fp64_u64_add(
    result,
    fp64_u64_shift_left(vec2u(0u, crossProduct), 16u)
  );
  result = fp64_u64_add(result, vec2u(highProduct, 0u));
  return result;
}

fn fp64_two_prod_integer_bits(aBits: u32, bBits: u32) -> vec2u {
  let a = fp64_decode_f32_bits(aBits);
  let b = fp64_decode_f32_bits(bBits);
  let resultSign = a.sign ^ b.sign;

  if (a.isNan || b.isNan || ((a.isZero || b.isZero) && (a.isInf || b.isInf))) {
    return vec2u(0x7fc00000u, 0u);
  }
  if (a.isInf || b.isInf) {
    return vec2u((resultSign << 31u) | 0x7f800000u, resultSign << 31u);
  }
  if (a.isZero || b.isZero) {
    return vec2u(resultSign << 31u, resultSign << 31u);
  }

  let magnitude = fp64_multiply_significands(a.significand, b.significand);
  return fp64_split_accumulator_bits(
    resultSign,
    magnitude,
    a.baseExponent + b.baseExponent
  );
}

fn fp64_two_prod_integer(a: f32, b: f32) -> vec2f {
  let resultBits = fp64_two_prod_integer_bits(bitcast<u32>(a), bitcast<u32>(b));
  return vec2f(bitcast<f32>(resultBits.x), bitcast<f32>(resultBits.y));
}

fn fp64_round_add_integer(a: f32, b: f32) -> f32 {
  return fp64_two_sum_integer(a, b).x;
}

fn fp64_round_mul_integer(a: f32, b: f32) -> f32 {
  return fp64_two_prod_integer(a, b).x;
}

#ifndef LUMA_FP64_PREDICATE_ONLY
fn fp64_f32_finite_exponent(value: Fp64F32Bits) -> i32 {
  let mostSignificantBit = 31u - countLeadingZeros(value.significand);
  return value.baseExponent + i32(mostSignificantBit);
}

fn fp64_scale_f32_integer(value: f32, exponent: i32) -> f32 {
  let decoded = fp64_decode_f32_bits(bitcast<u32>(value));
  if (decoded.isZero || decoded.isInf || decoded.isNan) {
    return value;
  }
  let resultBits = fp64_make_f32_bits_from_u64(
    decoded.sign,
    vec2u(0u, decoded.significand),
    decoded.baseExponent + exponent
  );
  return bitcast<f32>(resultBits);
}

// Divide normalized significands so the hardware operation cannot overflow,
// underflow, or flush a subnormal result. Reapply the exponent with integer
// packing, which also produces subnormal correction limbs without relying on
// floating-point arithmetic to preserve them.
fn fp64_divide_f32_integer(aValue: f32, bValue: f32) -> f32 {
  let a = fp64_decode_f32_bits(bitcast<u32>(aValue));
  let b = fp64_decode_f32_bits(bitcast<u32>(bValue));
  if (a.isZero || b.isZero || a.isInf || b.isInf || a.isNan || b.isNan) {
    return aValue / bValue;
  }

  let aMostSignificantBit = 31u - countLeadingZeros(a.significand);
  let bMostSignificantBit = 31u - countLeadingZeros(b.significand);
  let normalizedABits = fp64_make_f32_bits_from_u64(
    a.sign,
    vec2u(0u, a.significand),
    -i32(aMostSignificantBit)
  );
  let normalizedBBits = fp64_make_f32_bits_from_u64(
    b.sign,
    vec2u(0u, b.significand),
    -i32(bMostSignificantBit)
  );
  let normalizedQuotient = bitcast<f32>(normalizedABits) / bitcast<f32>(normalizedBBits);
  let quotient = fp64_decode_f32_bits(bitcast<u32>(normalizedQuotient));
  let exponentShift =
    a.baseExponent + i32(aMostSignificantBit) -
    b.baseExponent - i32(bMostSignificantBit);
  let quotientBits = fp64_make_f32_bits_from_u64(
    quotient.sign,
    vec2u(0u, quotient.significand),
    quotient.baseExponent + exponentShift
  );
  return bitcast<f32>(quotientBits);
}
#endif

#ifndef LUMA_FP64_PREDICATE_ONLY
fn split(a: f32) -> vec2f {
  let aBits = bitcast<u32>(a);
  let decoded = fp64_decode_f32_bits(aBits);
  if (decoded.isZero || decoded.isInf || decoded.isNan) {
    return vec2f(a, 0.0);
  }

  var roundedHigh = decoded.significand >> 12u;
  let remainder = decoded.significand & 0xfffu;
  if (remainder > 0x800u || (remainder == 0x800u && (roundedHigh & 1u) == 1u)) {
    roundedHigh = roundedHigh + 1u;
  }
  var highMagnitude = vec2u(0u, roundedHigh << 12u);
  var highBits = fp64_make_f32_bits_from_u64(
    decoded.sign,
    highMagnitude,
    decoded.baseExponent
  );
  // Rounding the high limb of a maximum-exponent value can overflow even
  // though the original value is finite. Truncate only in that boundary case
  // so split remains an exact finite decomposition.
  if (fp64_decode_f32_bits(highBits).isInf) {
    roundedHigh = decoded.significand >> 12u;
    highMagnitude = vec2u(0u, roundedHigh << 12u);
    highBits = fp64_make_f32_bits_from_u64(
      decoded.sign,
      highMagnitude,
      decoded.baseExponent
    );
  }
  let lowBits = fp64_make_residual_f32_bits(
    decoded.sign,
    vec2u(0u, decoded.significand),
    decoded.baseExponent,
    highBits
  );
  return vec2f(bitcast<f32>(highBits), bitcast<f32>(lowBits));
}

fn split2(a: vec2f) -> vec2f {
  var result = split(a.x);
  result.y = fp64_round_add_integer(result.y, a.y);
  return result;
}
#endif

#ifndef LUMA_FP64_PREDICATE_ONLY
fn quickTwoSum(a: f32, b: f32) -> vec2f {
  return fp64_two_sum_integer(a, b);
}
#endif

fn twoSum(a: f32, b: f32) -> vec2f {
  return fp64_two_sum_integer(a, b);
}

fn twoSub(a: f32, b: f32) -> vec2f {
  let bBits = bitcast<u32>(b) ^ 0x80000000u;
  let resultBits = fp64_two_sum_integer_bits(bitcast<u32>(a), bBits);
  return vec2f(bitcast<f32>(resultBits.x), bitcast<f32>(resultBits.y));
}

#ifndef LUMA_FP64_PREDICATE_ONLY
fn twoSqr(a: f32) -> vec2f {
  return fp64_two_prod_integer(a, a);
}

fn twoProd(a: f32, b: f32) -> vec2f {
  return fp64_two_prod_integer(a, b);
}
#endif

fn sum_fp64(a: vec2f, b: vec2f) -> vec2f {
  var sum = fp64_two_sum_integer(a.x, b.x);
  let lowSum = fp64_two_sum_integer(a.y, b.y);
  sum.y = fp64_round_add_integer(sum.y, lowSum.x);
  sum = fp64_two_sum_integer(sum.x, sum.y);
  sum.y = fp64_round_add_integer(sum.y, lowSum.y);
  return fp64_two_sum_integer(sum.x, sum.y);
}

fn sub_fp64(a: vec2f, b: vec2f) -> vec2f {
  let negatedB = vec2f(
    bitcast<f32>(bitcast<u32>(b.x) ^ 0x80000000u),
    bitcast<f32>(bitcast<u32>(b.y) ^ 0x80000000u)
  );
  return sum_fp64(a, negatedB);
}

fn mul_fp64(a: vec2f, b: vec2f) -> vec2f {
  var product = fp64_two_prod_integer(a.x, b.x);
  let crossProduct1 = fp64_round_mul_integer(a.x, b.y);
  product.y = fp64_round_add_integer(product.y, crossProduct1);
  product = fp64_two_sum_integer(product.x, product.y);
  let crossProduct2 = fp64_round_mul_integer(a.y, b.x);
  product.y = fp64_round_add_integer(product.y, crossProduct2);
  return fp64_two_sum_integer(product.x, product.y);
}

#ifndef LUMA_FP64_PREDICATE_ONLY
fn fp64_scale_fp64_integer(value: vec2f, exponent: i32) -> vec2f {
  let high = fp64_scale_f32_integer(value.x, exponent);
  let low = fp64_scale_f32_integer(value.y, exponent);
  return sum_fp64(vec2f(high, 0.0), vec2f(low, 0.0));
}

fn fp64_div_fp64_normalized(a: vec2f, b: vec2f) -> vec2f {
  let quotientHigh = fp64_divide_f32_integer(a.x, b.x);
  var quotient = vec2f(quotientHigh, 0.0);

  let remainder = sub_fp64(a, mul_fp64(b, quotient));
  let quotientLow = fp64_divide_f32_integer(remainder.x, b.x);
  quotient = sum_fp64(quotient, vec2f(quotientLow, 0.0));

  let secondRemainder = sub_fp64(a, mul_fp64(b, quotient));
  let correction = fp64_divide_f32_integer(secondRemainder.x, b.x);
  return sum_fp64(quotient, vec2f(correction, 0.0));
}

fn div_fp64(a: vec2f, b: vec2f) -> vec2f {
  let decodedA = fp64_decode_f32_bits(bitcast<u32>(a.x));
  let decodedB = fp64_decode_f32_bits(bitcast<u32>(b.x));
  if (
    decodedA.isZero || decodedB.isZero ||
    decodedA.isInf || decodedB.isInf ||
    decodedA.isNan || decodedB.isNan
  ) {
    return fp64_div_fp64_normalized(a, b);
  }

  let exponentA = fp64_f32_finite_exponent(decodedA);
  let exponentB = fp64_f32_finite_exponent(decodedB);
  // Correct the quotient near unity so b * q and the remainder stay clear of
  // both f32 underflow and overflow. The exponent difference is applied once.
  let normalizedA = fp64_scale_fp64_integer(a, -exponentA);
  let normalizedB = fp64_scale_fp64_integer(b, -exponentB);
  let normalizedQuotient = fp64_div_fp64_normalized(normalizedA, normalizedB);
  return fp64_scale_fp64_integer(normalizedQuotient, exponentA - exponentB);
}

fn fp64_sqrt_fp64_normalized(a: vec2f) -> vec2f {
  let estimate = sqrt(a.x);
  let difference = sub_fp64(a, fp64_two_prod_integer(estimate, estimate)).x;
  let denominator = fp64_round_add_integer(estimate, estimate);
  let correction = fp64_divide_f32_integer(difference, denominator);
  return sum_fp64(vec2f(estimate, 0.0), vec2f(correction, 0.0));
}

fn sqrt_fp64(a: vec2f) -> vec2f {
  let decoded = fp64_decode_f32_bits(bitcast<u32>(a.x));
  let decodedLow = fp64_decode_f32_bits(bitcast<u32>(a.y));
  if (decoded.isZero && decodedLow.isZero) {
    return vec2f(0.0, 0.0);
  }
  if (decoded.sign == 1u) {
    let nanValue = fp64_nan(a.x);
    return vec2f(nanValue, nanValue);
  }

  if (decoded.isInf || decoded.isNan) {
    return fp64_sqrt_fp64_normalized(a);
  }
  let exponent = fp64_f32_finite_exponent(decoded);
  // An even scale lets the final square-root rescale use an integer exponent.
  let evenExponent = exponent - (exponent & 1);
  let normalizedA = fp64_scale_fp64_integer(a, -evenExponent);
  let normalizedRoot = fp64_sqrt_fp64_normalized(normalizedA);
  return fp64_scale_fp64_integer(normalizedRoot, evenExponent / 2);
}
#endif

#else
fn split(a: f32) -> vec2f {
  let splitValue = prevent_fp64_optimization(fp64arithmetic.SPLIT + fp64_runtime_zero());
  let t = prevent_fp64_optimization(a * splitValue);
  let temp = prevent_fp64_optimization(t - a);
  let aHi = prevent_fp64_optimization(t - temp);
  let aLo = prevent_fp64_optimization(a - aHi);
  return vec2f(aHi, aLo);
}

fn split2(a: vec2f) -> vec2f {
  var b = split(a.x);
  b.y = b.y + a.y;
  return b;
}

fn quickTwoSum(a: f32, b: f32) -> vec2f {
#ifdef LUMA_FP64_CODE_ELIMINATION_WORKAROUND
  let sum = prevent_fp64_optimization((a + b) * fp64arithmetic.ONE);
  let err = prevent_fp64_optimization(b - (sum - a) * fp64arithmetic.ONE);
#else
  let sum = prevent_fp64_optimization(a + b);
  let err = prevent_fp64_optimization(b - (sum - a));
#endif
  return vec2f(sum, err);
}

fn twoSum(a: f32, b: f32) -> vec2f {
  let s = prevent_fp64_optimization(a + b);
#ifdef LUMA_FP64_CODE_ELIMINATION_WORKAROUND
  let v = prevent_fp64_optimization((s * fp64arithmetic.ONE - a) * fp64arithmetic.ONE);
  let err =
    prevent_fp64_optimization((a - (s - v) * fp64arithmetic.ONE) *
      fp64arithmetic.ONE *
      fp64arithmetic.ONE *
      fp64arithmetic.ONE) +
    prevent_fp64_optimization(b - v);
#else
  let v = prevent_fp64_optimization(s - a);
  let err = prevent_fp64_optimization(a - (s - v)) + prevent_fp64_optimization(b - v);
#endif
  return vec2f(s, err);
}

fn twoSub(a: f32, b: f32) -> vec2f {
  let s = prevent_fp64_optimization(a - b);
#ifdef LUMA_FP64_CODE_ELIMINATION_WORKAROUND
  let v = prevent_fp64_optimization((s * fp64arithmetic.ONE - a) * fp64arithmetic.ONE);
  let err =
    prevent_fp64_optimization((a - (s - v) * fp64arithmetic.ONE) *
      fp64arithmetic.ONE *
      fp64arithmetic.ONE *
      fp64arithmetic.ONE) -
    prevent_fp64_optimization(b + v);
#else
  let v = prevent_fp64_optimization(s - a);
  let err = prevent_fp64_optimization(a - (s - v)) - prevent_fp64_optimization(b + v);
#endif
  return vec2f(s, err);
}

fn twoSqr(a: f32) -> vec2f {
  let prod = prevent_fp64_optimization(a * a);
  let aFp64 = split(a);
  let highProduct = prevent_fp64_optimization(aFp64.x * aFp64.x);
  let crossProduct = prevent_fp64_optimization(2.0 * aFp64.x * aFp64.y);
  let lowProduct = prevent_fp64_optimization(aFp64.y * aFp64.y);
#ifdef LUMA_FP64_CODE_ELIMINATION_WORKAROUND
  let err =
    (prevent_fp64_optimization(highProduct - prod) * fp64arithmetic.ONE +
      crossProduct * fp64arithmetic.ONE * fp64arithmetic.ONE) +
    lowProduct * fp64arithmetic.ONE * fp64arithmetic.ONE * fp64arithmetic.ONE;
#else
  let err = ((prevent_fp64_optimization(highProduct - prod) + crossProduct) + lowProduct);
#endif
  return vec2f(prod, err);
}

fn twoProd(a: f32, b: f32) -> vec2f {
  let prod = prevent_fp64_optimization(a * b);
  let aFp64 = split(a);
  let bFp64 = split(b);
  let highProduct = prevent_fp64_optimization(aFp64.x * bFp64.x);
  let crossProduct1 = prevent_fp64_optimization(aFp64.x * bFp64.y);
  let crossProduct2 = prevent_fp64_optimization(aFp64.y * bFp64.x);
  let lowProduct = prevent_fp64_optimization(aFp64.y * bFp64.y);
#ifdef LUMA_FP64_CODE_ELIMINATION_WORKAROUND
  let err1 = (highProduct - prod) * fp64arithmetic.ONE;
  let err2 = crossProduct1 * fp64arithmetic.ONE * fp64arithmetic.ONE;
  let err3 = crossProduct2 * fp64arithmetic.ONE * fp64arithmetic.ONE * fp64arithmetic.ONE;
  let err4 =
    lowProduct *
    fp64arithmetic.ONE *
    fp64arithmetic.ONE *
    fp64arithmetic.ONE *
    fp64arithmetic.ONE;
#else
  let err1 = highProduct - prod;
  let err2 = crossProduct1;
  let err3 = crossProduct2;
  let err4 = lowProduct;
#endif
  let err12InputA = prevent_fp64_optimization(err1);
  let err12InputB = prevent_fp64_optimization(err2);
  let err12 = prevent_fp64_optimization(err12InputA + err12InputB);
  let err123InputA = prevent_fp64_optimization(err12);
  let err123InputB = prevent_fp64_optimization(err3);
  let err123 = prevent_fp64_optimization(err123InputA + err123InputB);
  let err1234InputA = prevent_fp64_optimization(err123);
  let err1234InputB = prevent_fp64_optimization(err4);
  let err = prevent_fp64_optimization(err1234InputA + err1234InputB);
  return vec2f(prod, err);
}

fn sum_fp64(a: vec2f, b: vec2f) -> vec2f {
  var s = twoSum(a.x, b.x);
  let t = twoSum(a.y, b.y);
  s.y = prevent_fp64_optimization(s.y + t.x);
  s = quickTwoSum(s.x, s.y);
  s.y = prevent_fp64_optimization(s.y + t.y);
  s = quickTwoSum(s.x, s.y);
  return s;
}

fn sub_fp64(a: vec2f, b: vec2f) -> vec2f {
  var s = twoSub(a.x, b.x);
  let t = twoSub(a.y, b.y);
  s.y = prevent_fp64_optimization(s.y + t.x);
  s = quickTwoSum(s.x, s.y);
  s.y = prevent_fp64_optimization(s.y + t.y);
  s = quickTwoSum(s.x, s.y);
  return s;
}

fn mul_fp64(a: vec2f, b: vec2f) -> vec2f {
  var prod = twoProd(a.x, b.x);
  let crossProduct1 = prevent_fp64_optimization(a.x * b.y);
  prod.y = prevent_fp64_optimization(prod.y + crossProduct1);
#ifdef LUMA_FP64_HIGH_BITS_OVERFLOW_WORKAROUND
  prod = split2(prod);
#endif
  prod = quickTwoSum(prod.x, prod.y);
  let crossProduct2 = prevent_fp64_optimization(a.y * b.x);
  prod.y = prevent_fp64_optimization(prod.y + crossProduct2);
#ifdef LUMA_FP64_HIGH_BITS_OVERFLOW_WORKAROUND
  prod = split2(prod);
#endif
  prod = quickTwoSum(prod.x, prod.y);
  return prod;
}

#ifndef LUMA_FP64_PREDICATE_ONLY
fn div_fp64(a: vec2f, b: vec2f) -> vec2f {
  let xn = prevent_fp64_optimization(1.0 / b.x);
  let yn = mul_fp64(a, vec2f(xn, fp64_runtime_zero()));
  let diff = prevent_fp64_optimization(sub_fp64(a, mul_fp64(b, yn)).x);
  let prod = twoProd(xn, diff);
  return sum_fp64(yn, prod);
}

fn sqrt_fp64(a: vec2f) -> vec2f {
  if (a.x == 0.0 && a.y == 0.0) {
    return vec2f(0.0, 0.0);
  }
  if (a.x < 0.0) {
    let nanValue = fp64_nan(a.x);
    return vec2f(nanValue, nanValue);
  }

  let x = prevent_fp64_optimization(1.0 / sqrt(a.x));
  let yn = prevent_fp64_optimization(a.x * x);
#ifdef LUMA_FP64_CODE_ELIMINATION_WORKAROUND
  let ynSqr = twoSqr(yn) * fp64arithmetic.ONE;
#else
  let ynSqr = twoSqr(yn);
#endif
  let diff = prevent_fp64_optimization(sub_fp64(a, ynSqr).x);
  let prod = twoProd(prevent_fp64_optimization(x * 0.5), diff);
#ifdef LUMA_FP64_HIGH_BITS_OVERFLOW_WORKAROUND
  return sum_fp64(split(yn), prod);
#else
  return sum_fp64(vec2f(yn, 0.0), prod);
#endif
}
#endif
#endif

#ifndef LUMA_FP64_PREDICATE_ONLY
fn fp64_f32_bits_is_nan(bits: u32) -> bool {
  return (bits & 0x7fffffffu) > 0x7f800000u;
}

fn fp64_f32_bits_is_inf(bits: u32) -> bool {
  return (bits & 0x7fffffffu) == 0x7f800000u;
}

fn fp64_compare_f32_bits(aBits: u32, bBits: u32) -> i32 {
  let aMagnitude = aBits & 0x7fffffffu;
  let bMagnitude = bBits & 0x7fffffffu;
  if (aMagnitude == 0u && bMagnitude == 0u) {
    return 0;
  }
  let aSign = aBits >> 31u;
  let bSign = bBits >> 31u;
  if (aSign != bSign) {
    return select(1, -1, aSign == 1u);
  }
  if (aMagnitude == bMagnitude) {
    return 0;
  }
  let magnitudeComparison = select(-1, 1, aMagnitude > bMagnitude);
  return select(magnitudeComparison, -magnitudeComparison, aSign == 1u);
}

// Normalize an arbitrary pair of finite f32 limbs with integer accumulation.
// This is independent of LUMA_FP64_INTEGER_ARITHMETIC and canonicalizes every
// representation of zero to vec2f(+0.0, +0.0).
fn normalize_fp64(value: vec2f) -> vec2f {
  let resultBits = fp64_add_raw_f32_bits(bitcast<u32>(value.x), bitcast<u32>(value.y));
  return vec2f(bitcast<f32>(resultBits.x), bitcast<f32>(resultBits.y));
}

fn is_nan_fp64(value: vec2f) -> bool {
  let normalized = normalize_fp64(value);
  return fp64_f32_bits_is_nan(bitcast<u32>(normalized.x)) ||
    fp64_f32_bits_is_nan(bitcast<u32>(normalized.y));
}

fn is_finite_fp64(value: vec2f) -> bool {
  let normalized = normalize_fp64(value);
  let highBits = bitcast<u32>(normalized.x);
  let lowBits = bitcast<u32>(normalized.y);
  return !fp64_f32_bits_is_nan(highBits) && !fp64_f32_bits_is_nan(lowBits) &&
    !fp64_f32_bits_is_inf(highBits) && !fp64_f32_bits_is_inf(lowBits);
}

// Returns -1, 0, or 1. NaN is unordered and returns 0; call is_nan_fp64 or
// is_finite_fp64 first when 0 must mean a finite zero.
fn sign_fp64(value: vec2f) -> i32 {
  let normalized = normalize_fp64(value);
  let highBits = bitcast<u32>(normalized.x);
  let lowBits = bitcast<u32>(normalized.y);
  if (fp64_f32_bits_is_nan(highBits) || fp64_f32_bits_is_nan(lowBits)) {
    return 0;
  }
  if ((highBits & 0x7fffffffu) != 0u) {
    return select(1, -1, (highBits >> 31u) == 1u);
  }
  if ((lowBits & 0x7fffffffu) != 0u) {
    return select(1, -1, (lowBits >> 31u) == 1u);
  }
  return 0;
}

// Compares double-single values and returns -1, 0, or 1. NaN is unordered
// and returns 0; callers that require equality semantics must first check
// is_nan_fp64 or is_finite_fp64.
fn compare_fp64(a: vec2f, b: vec2f) -> i32 {
  let normalizedA = normalize_fp64(a);
  let normalizedB = normalize_fp64(b);
  let aHighBits = bitcast<u32>(normalizedA.x);
  let aLowBits = bitcast<u32>(normalizedA.y);
  let bHighBits = bitcast<u32>(normalizedB.x);
  let bLowBits = bitcast<u32>(normalizedB.y);
  if (fp64_f32_bits_is_nan(aHighBits) || fp64_f32_bits_is_nan(aLowBits) ||
      fp64_f32_bits_is_nan(bHighBits) || fp64_f32_bits_is_nan(bLowBits)) {
    return 0;
  }
  let highComparison = fp64_compare_f32_bits(aHighBits, bHighBits);
  if (highComparison != 0) {
    return highComparison;
  }
  return fp64_compare_f32_bits(aLowBits, bLowBits);
}
#endif
`,fs:fr,vs:fr,defaultUniforms:{ONE:1,SPLIT:4097},uniformTypes:{ONE:`f32`,SPLIT:`f32`},fp64ify:or,fp64LowPart:sr,fp64ifyMatrix4:cr},mr={RGBA8UNORM:0,RGBA16FLOAT:1,RGBA32FLOAT:2},hr={rgba8unorm:4,rgba16float:8,rgba32float:16};mr.RGBA8UNORM,mr.RGBA16FLOAT,mr.RGBA32FLOAT,mr.RGBA8UNORM,mr.RGBA16FLOAT,mr.RGBA32FLOAT;var gr={useByteColors:`f32`},_r={useByteColors:!0};mr.RGBA8UNORM,hr.rgba8unorm/Uint32Array.BYTES_PER_ELEMENT,br(`colors`);var vr=br(`floatColors`);xr(`colors`);var yr=xr(`floatColors`);`${mr.RGBA8UNORM}${mr.RGBA16FLOAT}`;function br(e){return`\
layout(std140) uniform ${e}Uniforms {
  float useByteColors;
} ${e};

vec3 ${e}_normalize(vec3 inputColor) {
  return ${e}.useByteColors > 0.5 ? inputColor / 255.0 : inputColor;
}

vec4 ${e}_normalize(vec4 inputColor) {
  return ${e}.useByteColors > 0.5 ? inputColor / 255.0 : inputColor;
}

vec4 ${e}_premultiplyAlpha(vec4 inputColor) {
  return vec4(inputColor.rgb * inputColor.a, inputColor.a);
}

vec4 ${e}_unpremultiplyAlpha(vec4 inputColor) {
  return inputColor.a > 0.0 ? vec4(inputColor.rgb / inputColor.a, inputColor.a) : vec4(0.0);
}

vec4 ${e}_premultiply_alpha(vec4 inputColor) {
  return ${e}_premultiplyAlpha(inputColor);
}

vec4 ${e}_unpremultiply_alpha(vec4 inputColor) {
  return ${e}_unpremultiplyAlpha(inputColor);
}
`}function xr(e){return`\
struct ${e}Uniforms {
  useByteColors: f32
};

@group(0) @binding(auto) var<uniform> ${e} : ${e}Uniforms;

fn ${e}_normalize(inputColor: vec3<f32>) -> vec3<f32> {
  return select(inputColor, inputColor / 255.0, ${e}.useByteColors > 0.5);
}

fn ${e}_normalize4(inputColor: vec4<f32>) -> vec4<f32> {
  return select(inputColor, inputColor / 255.0, ${e}.useByteColors > 0.5);
}

fn ${e}_premultiplyAlpha(inputColor: vec4<f32>) -> vec4<f32> {
  return vec4<f32>(inputColor.rgb * inputColor.a, inputColor.a);
}

fn ${e}_unpremultiplyAlpha(inputColor: vec4<f32>) -> vec4<f32> {
  return select(
    vec4<f32>(0.0),
    vec4<f32>(inputColor.rgb / inputColor.a, inputColor.a),
    inputColor.a > 0.0
  );
}

fn ${e}_premultiply_alpha(inputColor: vec4<f32>) -> vec4<f32> {
  return ${e}_premultiplyAlpha(inputColor);
}

fn ${e}_unpremultiply_alpha(inputColor: vec4<f32>) -> vec4<f32> {
  return ${e}_unpremultiplyAlpha(inputColor);
}
`}var Sr={name:`floatColors`,props:{},uniforms:{},vs:vr,fs:vr,source:yr,uniformTypes:gr,defaultUniforms:_r},Cr={props:{},uniforms:{},name:`picking`,uniformTypes:{isActive:`f32`,isAttribute:`f32`,isHighlightActive:`f32`,useByteColors:`f32`,highlightedObjectColor:`vec3<f32>`,highlightColor:`vec4<f32>`},defaultUniforms:{isActive:!1,isAttribute:!1,isHighlightActive:!1,useByteColors:!0,highlightedObjectColor:[0,0,0],highlightColor:[0,1,1,1]},vs:`layout(std140) uniform pickingUniforms {
  float isActive;
  float isAttribute;
  float isHighlightActive;
  float useByteColors;
  vec3 highlightedObjectColor;
  vec4 highlightColor;
} picking;

out vec4 picking_vRGBcolor_Avalid;

// Normalize unsigned byte color to 0-1 range
vec3 picking_normalizeColor(vec3 color) {
  return picking.useByteColors > 0.5 ? color / 255.0 : color;
}

// Normalize unsigned byte color to 0-1 range
vec4 picking_normalizeColor(vec4 color) {
  return picking.useByteColors > 0.5 ? color / 255.0 : color;
}

bool picking_isColorZero(vec3 color) {
  return dot(color, vec3(1.0)) < 0.00001;
}

bool picking_isColorValid(vec3 color) {
  return dot(color, vec3(1.0)) > 0.00001;
}

// Check if this vertex is highlighted 
bool isVertexHighlighted(vec3 vertexColor) {
  vec3 highlightedObjectColor = picking_normalizeColor(picking.highlightedObjectColor);
  return
    bool(picking.isHighlightActive) && picking_isColorZero(abs(vertexColor - highlightedObjectColor));
}

// Set the current picking color
void picking_setPickingColor(vec3 pickingColor) {
  pickingColor = picking_normalizeColor(pickingColor);

  if (bool(picking.isActive)) {
    // Use alpha as the validity flag. If pickingColor is [0, 0, 0] fragment is non-pickable
    picking_vRGBcolor_Avalid.a = float(picking_isColorValid(pickingColor));

    if (!bool(picking.isAttribute)) {
      // Stores the picking color so that the fragment shader can render it during picking
      picking_vRGBcolor_Avalid.rgb = pickingColor;
    }
  } else {
    // Do the comparison with selected item color in vertex shader as it should mean fewer compares
    picking_vRGBcolor_Avalid.a = float(isVertexHighlighted(pickingColor));
  }
}

void picking_setPickingAttribute(float value) {
  if (bool(picking.isAttribute)) {
    picking_vRGBcolor_Avalid.r = value;
  }
}

void picking_setPickingAttribute(vec2 value) {
  if (bool(picking.isAttribute)) {
    picking_vRGBcolor_Avalid.rg = value;
  }
}

void picking_setPickingAttribute(vec3 value) {
  if (bool(picking.isAttribute)) {
    picking_vRGBcolor_Avalid.rgb = value;
  }
}
`,fs:`layout(std140) uniform pickingUniforms {
  float isActive;
  float isAttribute;
  float isHighlightActive;
  float useByteColors;
  vec3 highlightedObjectColor;
  vec4 highlightColor;
} picking;

in vec4 picking_vRGBcolor_Avalid;

/*
 * Returns highlight color if this item is selected.
 */
vec4 picking_filterHighlightColor(vec4 color) {
  // If we are still picking, we don't highlight
  if (picking.isActive > 0.5) {
    return color;
  }

  bool selected = bool(picking_vRGBcolor_Avalid.a);

  if (selected) {
    // Blend in highlight color based on its alpha value
    float highLightAlpha = picking.highlightColor.a;
    float blendedAlpha = highLightAlpha + color.a * (1.0 - highLightAlpha);
    float highLightRatio = highLightAlpha / blendedAlpha;

    vec3 blendedRGB = mix(color.rgb, picking.highlightColor.rgb, highLightRatio);
    return vec4(blendedRGB, blendedAlpha);
  } else {
    return color;
  }
}

/*
 * Returns picking color if picking enabled else unmodified argument.
 */
vec4 picking_filterPickingColor(vec4 color) {
  if (bool(picking.isActive)) {
    if (picking_vRGBcolor_Avalid.a == 0.0) {
      discard;
    }
    return picking_vRGBcolor_Avalid;
  }
  return color;
}

/*
 * Returns picking color if picking is enabled if not
 * highlight color if this item is selected, otherwise unmodified argument.
 */
vec4 picking_filterColor(vec4 color) {
  vec4 highlightColor = picking_filterHighlightColor(color);
  return picking_filterPickingColor(highlightColor);
}
`,getUniforms:wr};function wr(e={},t){let n={},r=lr(e.useByteColors,!0);return e.highlightedObjectColor===void 0||(e.highlightedObjectColor===null?n.isHighlightActive=!1:(n.isHighlightActive=!0,n.highlightedObjectColor=e.highlightedObjectColor.slice(0,3))),e.highlightColor&&(n.highlightColor=dr(e.highlightColor,r)),e.isActive!==void 0&&(n.isActive=!!e.isActive,n.isAttribute=!!e.isAttribute),e.useByteColors!==void 0&&(n.useByteColors=!!e.useByteColors),n}var Tr={props:{},uniforms:{},bindings:{},name:`skin`,bindingLayout:[{name:`skin`,group:0},{name:`skinJointMatrices`,group:0,visibility:1}],dependencies:[],source:`
struct skinUniforms {
  jointMatrix: array<mat4x4<f32>, 64>,
};

@group(0) @binding(auto) var<uniform> skin: skinUniforms;

#ifdef HAS_INSTANCED_SKIN
@group(0) @binding(auto) var<storage, read> skinJointMatrices: array<mat4x4<f32>>;

fn getInstancedSkinMatrix(
  weights: vec4f,
  joints: vec4u,
  instanceIndex: u32,
  jointsPerInstance: u32
) -> mat4x4<f32> {
  let firstJoint = instanceIndex * jointsPerInstance;
  return (weights.x * skinJointMatrices[firstJoint + joints.x])
       + (weights.y * skinJointMatrices[firstJoint + joints.y])
       + (weights.z * skinJointMatrices[firstJoint + joints.z])
       + (weights.w * skinJointMatrices[firstJoint + joints.w]);
}
#endif

fn getSkinMatrix(weights: vec4f, joints: vec4u) -> mat4x4<f32> {
  return (weights.x * skin.jointMatrix[joints.x])
       + (weights.y * skin.jointMatrix[joints.y])
       + (weights.z * skin.jointMatrix[joints.z])
       + (weights.w * skin.jointMatrix[joints.w]);
}
`,vs:`
layout(std140) uniform skinUniforms {
  mat4 jointMatrix[SKIN_MAX_JOINTS];
} skin;

#ifdef HAS_INSTANCED_SKIN
uniform highp sampler2D skinJointMatrices;

mat4 getInstancedJointMatrix(uint jointIndex, uint instanceIndex) {
  int firstColumn = int(jointIndex * 4u);
  int row = int(instanceIndex);
  return mat4(
    texelFetch(skinJointMatrices, ivec2(firstColumn, row), 0),
    texelFetch(skinJointMatrices, ivec2(firstColumn + 1, row), 0),
    texelFetch(skinJointMatrices, ivec2(firstColumn + 2, row), 0),
    texelFetch(skinJointMatrices, ivec2(firstColumn + 3, row), 0)
  );
}

mat4 getInstancedSkinMatrix(
  vec4 weights,
  uvec4 joints,
  uint instanceIndex,
  uint jointsPerInstance
) {
  return (weights.x * getInstancedJointMatrix(joints.x, instanceIndex))
       + (weights.y * getInstancedJointMatrix(joints.y, instanceIndex))
       + (weights.z * getInstancedJointMatrix(joints.z, instanceIndex))
       + (weights.w * getInstancedJointMatrix(joints.w, instanceIndex));
}
#endif

mat4 getSkinMatrix(vec4 weights, uvec4 joints) {
  return (weights.x * skin.jointMatrix[joints.x])
       + (weights.y * skin.jointMatrix[joints.y])
       + (weights.z * skin.jointMatrix[joints.z])
       + (weights.w * skin.jointMatrix[joints.w]);
}

`,fs:``,defines:{SKIN_MAX_JOINTS:64},getUniforms:(e={},t)=>{let{jointMatrices:n,skinJointMatrices:r,scenegraphsFromGLTF:i,skinIndex:a=0,meshWorldMatrix:o}=e,s=r?{skinJointMatrices:r}:{};if(n)return{jointMatrix:Er(n),...s};let c=i?.gltf?.skins?.[a];if(!c)return{jointMatrix:[],...s};let{inverseBindMatrices:l,joints:u,skeleton:d}=c,f=i.gltfNodeIndexToNodeMap,p=new Map,m=d===void 0?void 0:f?.get(d),h=m?[m]:i.scenes||[];for(let e of h)e.preorderTraversal((e,{worldMatrix:t})=>{p.set(e.id,t)});let g=o?new R(o).invert():null,_=new Float32Array(1024),v=l?.value;for(let e=0;e<Math.min(u.length,64);e++){let t=f?.get(u[e]);if(!t)continue;let n=p.get(t.id)||t.matrix,r=g?new R(g).multiplyRight(n):new R(n);v&&v.length>=(e+1)*16&&r.multiplyRight(new R(Array.from(v.slice(e*16,(e+1)*16)))),_.set(r,e*16)}return{jointMatrix:_,...s}},uniformTypes:{jointMatrix:[`mat4x4<f32>`,64]}};function Er(e){let t=new Float32Array(1024);return t.set(e instanceof Float32Array?e.subarray(0,t.length):e.slice(0,t.length)),t}var Dr={name:`gpuAnimation`,props:{},uniforms:{},bindings:{},source:`
#ifdef HAS_GPU_CROWD_ANIMATION
@group(0) @binding(auto) var<storage, read> gpuAnimationFrames: array<vec4f>;

fn readGPUAnimationFrame(frame: u32, offset: u32, frameStride: u32) -> vec4f {
  return gpuAnimationFrames[frame * frameStride + offset];
}

fn sampleGPUAnimationFrame(
  frames: vec4f,
  blend: vec4f,
  offset: u32,
  frameStride: u32
) -> vec4f {
  let first = mix(
    readGPUAnimationFrame(u32(frames.x), offset, frameStride),
    readGPUAnimationFrame(u32(frames.y), offset, frameStride),
    frames.z
  );
  if (blend.w <= 0.0) {
    return first;
  }
  let second = mix(
    readGPUAnimationFrame(u32(blend.x), offset, frameStride),
    readGPUAnimationFrame(u32(blend.y), offset, frameStride),
    blend.z
  );
  return mix(first, second, blend.w);
}

fn sampleGPUAnimationMatrix(
  frames: vec4f,
  blend: vec4f,
  firstColumn: u32,
  frameStride: u32
) -> mat4x4f {
  return mat4x4f(
    sampleGPUAnimationFrame(frames, blend, firstColumn, frameStride),
    sampleGPUAnimationFrame(frames, blend, firstColumn + 1u, frameStride),
    sampleGPUAnimationFrame(frames, blend, firstColumn + 2u, frameStride),
    sampleGPUAnimationFrame(frames, blend, firstColumn + 3u, frameStride)
  );
}

fn getGPUAnimatedSkinMatrix(
  weights: vec4f,
  joints: vec4u,
  frames: vec4f,
  blend: vec4f,
  frameStride: u32
) -> mat4x4f {
  return weights.x * sampleGPUAnimationMatrix(frames, blend, 4u + joints.x * 4u, frameStride)
       + weights.y * sampleGPUAnimationMatrix(frames, blend, 4u + joints.y * 4u, frameStride)
       + weights.z * sampleGPUAnimationMatrix(frames, blend, 4u + joints.z * 4u, frameStride)
       + weights.w * sampleGPUAnimationMatrix(frames, blend, 4u + joints.w * 4u, frameStride);
}
#endif

#ifdef HAS_INSTANCED_MORPH
@group(0) @binding(auto) var<storage, read> gpuMorphTargets: array<vec4f>;

#ifndef HAS_GPU_CROWD_ANIMATION
@group(0) @binding(auto) var<storage, read> gpuMorphWeights: array<vec4f>;
#endif

fn getGPUCrowdMorphWeight(
  instanceIndex: u32,
  targetIndex: u32,
  targetCount: u32,
  jointsPerInstance: u32,
  frames: vec4f,
  blend: vec4f,
  frameStride: u32
) -> f32 {
#ifdef HAS_GPU_CROWD_ANIMATION
  let offset = 4u + jointsPerInstance * 4u + targetIndex;
  return sampleGPUAnimationFrame(frames, blend, offset, frameStride).x;
#else
  let packedCount = (targetCount + 3u) / 4u;
  let packedWeights = gpuMorphWeights[instanceIndex * packedCount + targetIndex / 4u];
  return packedWeights[targetIndex % 4u];
#endif
}

fn getGPUCrowdMorphDelta(
  instanceIndex: u32,
  vertexIndex: u32,
  attributeIndex: u32,
  vertexCount: u32,
  targetCount: u32,
  jointsPerInstance: u32,
  frames: vec4f,
  blend: vec4f,
  frameStride: u32
) -> vec3f {
  var result = vec3f(0.0);
  for (var targetIndex = 0u; targetIndex < targetCount; targetIndex++) {
    let weight = getGPUCrowdMorphWeight(
      instanceIndex,
      targetIndex,
      targetCount,
      jointsPerInstance,
      frames,
      blend,
      frameStride
    );
    let offset = (targetIndex * 3u + attributeIndex) * vertexCount + vertexIndex;
    result += gpuMorphTargets[offset].xyz * weight;
  }
  return result;
}
#endif
`,vs:`
#ifdef HAS_GPU_CROWD_ANIMATION
uniform highp sampler2D gpuAnimationFrames;

vec4 sampleGPUAnimationFrame(vec4 frames, vec4 blend, int offset) {
  vec4 first = mix(
    texelFetch(gpuAnimationFrames, ivec2(offset, int(frames.x)), 0),
    texelFetch(gpuAnimationFrames, ivec2(offset, int(frames.y)), 0),
    frames.z
  );
  if (blend.w <= 0.0) {
    return first;
  }
  vec4 second = mix(
    texelFetch(gpuAnimationFrames, ivec2(offset, int(blend.x)), 0),
    texelFetch(gpuAnimationFrames, ivec2(offset, int(blend.y)), 0),
    blend.z
  );
  return mix(first, second, blend.w);
}

mat4 sampleGPUAnimationMatrix(vec4 frames, vec4 blend, int firstColumn) {
  return mat4(
    sampleGPUAnimationFrame(frames, blend, firstColumn),
    sampleGPUAnimationFrame(frames, blend, firstColumn + 1),
    sampleGPUAnimationFrame(frames, blend, firstColumn + 2),
    sampleGPUAnimationFrame(frames, blend, firstColumn + 3)
  );
}

mat4 getGPUAnimatedSkinMatrix(vec4 weights, uvec4 joints, vec4 frames, vec4 blend) {
  return weights.x * sampleGPUAnimationMatrix(frames, blend, 4 + int(joints.x) * 4)
       + weights.y * sampleGPUAnimationMatrix(frames, blend, 4 + int(joints.y) * 4)
       + weights.z * sampleGPUAnimationMatrix(frames, blend, 4 + int(joints.z) * 4)
       + weights.w * sampleGPUAnimationMatrix(frames, blend, 4 + int(joints.w) * 4);
}
#endif

#ifdef HAS_INSTANCED_MORPH
uniform highp sampler2D gpuMorphTargets;

#ifndef HAS_GPU_CROWD_ANIMATION
uniform highp sampler2D gpuMorphWeights;
#endif

float getGPUCrowdMorphWeight(
  uint instanceIndex,
  uint targetIndex,
  uint jointsPerInstance,
  vec4 frames,
  vec4 blend
) {
#ifdef HAS_GPU_CROWD_ANIMATION
  int offset = 4 + int(jointsPerInstance) * 4 + int(targetIndex);
  return sampleGPUAnimationFrame(frames, blend, offset).x;
#else
  vec4 packedWeights = texelFetch(
    gpuMorphWeights,
    ivec2(int(targetIndex / 4u), int(instanceIndex)),
    0
  );
  return packedWeights[int(targetIndex % 4u)];
#endif
}

vec3 getGPUCrowdMorphDelta(
  uint instanceIndex,
  uint vertexIndex,
  uint attributeIndex,
  uint targetCount,
  uint jointsPerInstance,
  vec4 frames,
  vec4 blend
) {
  vec3 result = vec3(0.0);
  for (uint targetIndex = 0u; targetIndex < targetCount; targetIndex++) {
    float weight = getGPUCrowdMorphWeight(
      instanceIndex,
      targetIndex,
      jointsPerInstance,
      frames,
      blend
    );
    result += texelFetch(
      gpuMorphTargets,
      ivec2(int(vertexIndex), int(targetIndex * 3u + attributeIndex)),
      0
    ).xyz * weight;
  }
  return result;
}
#endif
`,fs:``,bindingLayout:[{name:`gpuAnimationFrames`,group:0,visibility:1},{name:`gpuMorphTargets`,group:0,visibility:1},{name:`gpuMorphWeights`,group:0,visibility:1}],getUniforms(e={}){return e}},Or=`precision highp int;

// #if (defined(SHADER_TYPE_FRAGMENT) && defined(LIGHTING_FRAGMENT)) || (defined(SHADER_TYPE_VERTEX) && defined(LIGHTING_VERTEX))
struct AmbientLight {
  vec3 color;
};

struct PointLight {
  vec3 color;
  vec3 position;
  vec3 attenuation; // 2nd order x:Constant-y:Linear-z:Exponential
};

struct SpotLight {
  vec3 color;
  vec3 position;
  vec3 direction;
  vec3 attenuation;
  vec2 coneCos;
};

struct DirectionalLight {
  vec3 color;
  vec3 direction;
};

struct UniformLight {
  vec3 color;
  vec3 position;
  vec3 direction;
  vec3 attenuation;
  vec2 coneCos;
};

layout(std140) uniform lightingUniforms {
  int enabled;
  int directionalLightCount;
  int pointLightCount;
  int spotLightCount;
  vec3 ambientColor;
  UniformLight lights[5];
} lighting;

PointLight lighting_getPointLight(int index) {
  UniformLight light = lighting.lights[index];
  return PointLight(light.color, light.position, light.attenuation);
}

SpotLight lighting_getSpotLight(int index) {
  UniformLight light = lighting.lights[lighting.pointLightCount + index];
  return SpotLight(light.color, light.position, light.direction, light.attenuation, light.coneCos);
}

DirectionalLight lighting_getDirectionalLight(int index) {
  UniformLight light =
    lighting.lights[lighting.pointLightCount + lighting.spotLightCount + index];
  return DirectionalLight(light.color, light.direction);
}

float getPointLightAttenuation(PointLight pointLight, float distance) {
  return pointLight.attenuation.x
       + pointLight.attenuation.y * distance
       + pointLight.attenuation.z * distance * distance;
}

float getSpotLightAttenuation(SpotLight spotLight, vec3 positionWorldspace) {
  vec3 light_direction = normalize(positionWorldspace - spotLight.position);
  float coneFactor = smoothstep(
    spotLight.coneCos.y,
    spotLight.coneCos.x,
    dot(normalize(spotLight.direction), light_direction)
  );
  float distanceAttenuation = getPointLightAttenuation(
    PointLight(spotLight.color, spotLight.position, spotLight.attenuation),
    distance(spotLight.position, positionWorldspace)
  );
  return distanceAttenuation / max(coneFactor, 0.0001);
}

// #endif
`,kr=`// #if (defined(SHADER_TYPE_FRAGMENT) && defined(LIGHTING_FRAGMENT)) || (defined(SHADER_TYPE_VERTEX) && defined(LIGHTING_VERTEX))
const MAX_LIGHTS: i32 = 5;

struct AmbientLight {
  color: vec3<f32>,
};

struct PointLight {
  color: vec3<f32>,
  position: vec3<f32>,
  attenuation: vec3<f32>, // 2nd order x:Constant-y:Linear-z:Exponential
};

struct SpotLight {
  color: vec3<f32>,
  position: vec3<f32>,
  direction: vec3<f32>,
  attenuation: vec3<f32>,
  coneCos: vec2<f32>,
};

struct DirectionalLight {
  color: vec3<f32>,
  direction: vec3<f32>,
};

struct UniformLight {
  color: vec3<f32>,
  position: vec3<f32>,
  direction: vec3<f32>,
  attenuation: vec3<f32>,
  coneCos: vec2<f32>,
};

struct lightingUniforms {
  enabled: i32,
  directionalLightCount: i32,
  pointLightCount: i32,
  spotLightCount: i32,
  ambientColor: vec3<f32>,
  lights: array<UniformLight, 5>,
};

@group(2) @binding(auto) var<uniform> lighting : lightingUniforms;

fn lighting_getPointLight(index: i32) -> PointLight {
  let light = lighting.lights[index];
  return PointLight(light.color, light.position, light.attenuation);
}

fn lighting_getSpotLight(index: i32) -> SpotLight {
  let light = lighting.lights[lighting.pointLightCount + index];
  return SpotLight(light.color, light.position, light.direction, light.attenuation, light.coneCos);
}

fn lighting_getDirectionalLight(index: i32) -> DirectionalLight {
  let light = lighting.lights[lighting.pointLightCount + lighting.spotLightCount + index];
  return DirectionalLight(light.color, light.direction);
}

fn getPointLightAttenuation(pointLight: PointLight, distance: f32) -> f32 {
  return pointLight.attenuation.x
       + pointLight.attenuation.y * distance
       + pointLight.attenuation.z * distance * distance;
}

fn getSpotLightAttenuation(spotLight: SpotLight, positionWorldspace: vec3<f32>) -> f32 {
  let lightDirection = normalize(positionWorldspace - spotLight.position);
  let coneFactor = smoothstep(
    spotLight.coneCos.y,
    spotLight.coneCos.x,
    dot(normalize(spotLight.direction), lightDirection)
  );
  let distanceAttenuation = getPointLightAttenuation(
    PointLight(spotLight.color, spotLight.position, spotLight.attenuation),
    distance(spotLight.position, positionWorldspace)
  );
  return distanceAttenuation / max(coneFactor, 0.0001);
}
`,Ar=5,jr={props:{},uniforms:{},name:`lighting`,defines:{},uniformTypes:{enabled:`i32`,directionalLightCount:`i32`,pointLightCount:`i32`,spotLightCount:`i32`,ambientColor:`vec3<f32>`,lights:[{color:`vec3<f32>`,position:`vec3<f32>`,direction:`vec3<f32>`,attenuation:`vec3<f32>`,coneCos:`vec2<f32>`},Ar]},defaultUniforms:Ir(),bindingLayout:[{name:`lighting`,group:2}],firstBindingSlot:0,source:kr,vs:Or,fs:Or,getUniforms:Mr};function Mr(e,t={}){if(e&&={...e},!e)return Ir();e.lights&&(e={...e,...Pr(e.lights),lights:void 0});let{useByteColors:n,ambientLight:r,pointLights:i,spotLights:a,directionalLights:o}=e||{};if(!(r||i&&i.length>0||a&&a.length>0||o&&o.length>0))return{...Ir(),enabled:0};let s={...Ir(),...Nr({useByteColors:n,ambientLight:r,pointLights:i,spotLights:a,directionalLights:o})};return e.enabled!==void 0&&(s.enabled=+!!e.enabled),s}function Nr({useByteColors:e,ambientLight:t,pointLights:n=[],spotLights:r=[],directionalLights:i=[]}){let a=Lr(),o=0,s=0,c=0,l=0;for(let t of n){if(o>=Ar)break;a[o]={...a[o],color:Fr(t,e),position:t.position,attenuation:t.attenuation||[1,0,0]},o++,s++}for(let t of r){if(o>=Ar)break;a[o]={...a[o],color:Fr(t,e),position:t.position,direction:t.direction,attenuation:t.attenuation||[1,0,0],coneCos:zr(t)},o++,c++}for(let t of i){if(o>=Ar)break;a[o]={...a[o],color:Fr(t,e),direction:t.direction},o++,l++}return n.length+r.length+i.length>Ar&&w.warn(`MAX_LIGHTS exceeded, truncating to ${Ar}`)(),{ambientColor:Fr(t,e),directionalLightCount:l,pointLightCount:s,spotLightCount:c,lights:a}}function Pr(e){let t={pointLights:[],spotLights:[],directionalLights:[]};for(let n of e||[])switch(n.type){case`ambient`:t.ambientLight=n;break;case`directional`:t.directionalLights?.push(n);break;case`point`:t.pointLights?.push(n);break;case`spot`:t.spotLights?.push(n);break;default:}return t}function Fr(e={},t){let{color:n=[0,0,0],intensity:r=1}=e;return ur(n,lr(t,!0)).map(e=>e*r)}function Ir(){return{enabled:1,directionalLightCount:0,pointLightCount:0,spotLightCount:0,ambientColor:[.1,.1,.1],lights:Lr()}}function Lr(){return Array.from({length:Ar},()=>Rr())}function Rr(){return{color:[1,1,1],position:[1,1,2],direction:[1,1,1],attenuation:[1,0,0],coneCos:[1,0]}}function zr(e){let t=e.innerConeAngle??0,n=e.outerConeAngle??Math.PI/4;return[Math.cos(t),Math.cos(n)]}var Br=`#ifdef USE_IBL
@group(2) @binding(auto) var pbr_diffuseEnvSampler: texture_cube<f32>;
@group(2) @binding(auto) var pbr_diffuseEnvSamplerSampler: sampler;
@group(2) @binding(auto) var pbr_specularEnvSampler: texture_cube<f32>;
@group(2) @binding(auto) var pbr_specularEnvSamplerSampler: sampler;
@group(2) @binding(auto) var pbr_brdfLUT: texture_2d<f32>;
@group(2) @binding(auto) var pbr_brdfLUTSampler: sampler;
#endif
`,Vr=`#ifdef USE_IBL
uniform samplerCube pbr_diffuseEnvSampler;
uniform samplerCube pbr_specularEnvSampler;
uniform sampler2D pbr_brdfLUT;
#endif
`,Hr={name:`ibl`,firstBindingSlot:32,bindingLayout:[{name:`pbr_diffuseEnvSampler`,group:2},{name:`pbr_specularEnvSampler`,group:2},{name:`pbr_brdfLUT`,group:2}],source:Br,vs:Vr,fs:Vr},Ur=`layout(std140) uniform phongMaterialUniforms {
  uniform bool unlit;
  uniform float ambient;
  uniform float diffuse;
  uniform float shininess;
  uniform vec3  specularColor;
} material;
`,Wr=`layout(std140) uniform phongMaterialUniforms {
  uniform bool unlit;
  uniform float ambient;
  uniform float diffuse;
  uniform float shininess;
  uniform vec3  specularColor;
} material;

vec3 lighting_getLightColor(vec3 surfaceColor, vec3 light_direction, vec3 view_direction, vec3 normal_worldspace, vec3 color) {
  vec3 halfway_direction = normalize(light_direction + view_direction);
  float lambertian = dot(light_direction, normal_worldspace);
  float specular = 0.0;
  if (lambertian > 0.0) {
    float specular_angle = max(dot(normal_worldspace, halfway_direction), 0.0);
    specular = pow(specular_angle, material.shininess);
  }
  lambertian = max(lambertian, 0.0);
  return (lambertian * material.diffuse * surfaceColor + specular * floatColors_normalize(material.specularColor)) * color;
}

vec3 lighting_getLightColor(vec3 surfaceColor, vec3 cameraPosition, vec3 position_worldspace, vec3 normal_worldspace) {
  vec3 lightColor = surfaceColor;

  if (material.unlit) {
    return surfaceColor;
  }

  if (lighting.enabled == 0) {
    return lightColor;
  }

  vec3 view_direction = normalize(cameraPosition - position_worldspace);
  lightColor = material.ambient * surfaceColor * lighting.ambientColor;

  for (int i = 0; i < lighting.pointLightCount; i++) {
    PointLight pointLight = lighting_getPointLight(i);
    vec3 light_position_worldspace = pointLight.position;
    vec3 light_direction = normalize(light_position_worldspace - position_worldspace);
    float light_attenuation = getPointLightAttenuation(pointLight, distance(light_position_worldspace, position_worldspace));
    lightColor += lighting_getLightColor(surfaceColor, light_direction, view_direction, normal_worldspace, pointLight.color / light_attenuation);
  }

  for (int i = 0; i < lighting.spotLightCount; i++) {
    SpotLight spotLight = lighting_getSpotLight(i);
    vec3 light_position_worldspace = spotLight.position;
    vec3 light_direction = normalize(light_position_worldspace - position_worldspace);
    float light_attenuation = getSpotLightAttenuation(spotLight, position_worldspace);
    lightColor += lighting_getLightColor(surfaceColor, light_direction, view_direction, normal_worldspace, spotLight.color / light_attenuation);
  }

  for (int i = 0; i < lighting.directionalLightCount; i++) {
    DirectionalLight directionalLight = lighting_getDirectionalLight(i);
    lightColor += lighting_getLightColor(surfaceColor, -directionalLight.direction, view_direction, normal_worldspace, directionalLight.color);
  }
  
  return lightColor;
}
`,Gr=`struct phongMaterialUniforms {
  unlit: u32,
  ambient: f32,
  diffuse: f32,
  shininess: f32,
  specularColor: vec3<f32>,
};

@group(3) @binding(auto) var<uniform> phongMaterial : phongMaterialUniforms;

fn lighting_getLightColor(surfaceColor: vec3<f32>, light_direction: vec3<f32>, view_direction: vec3<f32>, normal_worldspace: vec3<f32>, color: vec3<f32>) -> vec3<f32> {
  let halfway_direction: vec3<f32> = normalize(light_direction + view_direction);
  var lambertian: f32 = dot(light_direction, normal_worldspace);
  var specular: f32 = 0.0;
  if (lambertian > 0.0) {
    let specular_angle = max(dot(normal_worldspace, halfway_direction), 0.0);
    specular = pow(specular_angle, phongMaterial.shininess);
  }
  lambertian = max(lambertian, 0.0);
  return (
    lambertian * phongMaterial.diffuse * surfaceColor +
    specular * floatColors_normalize(phongMaterial.specularColor)
  ) * color;
}

fn lighting_getLightColor2(surfaceColor: vec3<f32>, cameraPosition: vec3<f32>, position_worldspace: vec3<f32>, normal_worldspace: vec3<f32>) -> vec3<f32> {
  var lightColor: vec3<f32> = surfaceColor;

  if (phongMaterial.unlit != 0u) {
    return surfaceColor;
  }

  if (lighting.enabled == 0) {
    return lightColor;
  }

  let view_direction: vec3<f32> = normalize(cameraPosition - position_worldspace);
  lightColor = phongMaterial.ambient * surfaceColor * lighting.ambientColor;

  for (var i: i32 = 0; i < lighting.pointLightCount; i++) {
    let pointLight: PointLight = lighting_getPointLight(i);
    let light_position_worldspace: vec3<f32> = pointLight.position;
    let light_direction: vec3<f32> = normalize(light_position_worldspace - position_worldspace);
    let light_attenuation = getPointLightAttenuation(
      pointLight,
      distance(light_position_worldspace, position_worldspace)
    );
    lightColor += lighting_getLightColor(
      surfaceColor,
      light_direction,
      view_direction,
      normal_worldspace,
      pointLight.color / light_attenuation
    );
  }

  for (var i: i32 = 0; i < lighting.spotLightCount; i++) {
    let spotLight: SpotLight = lighting_getSpotLight(i);
    let light_position_worldspace: vec3<f32> = spotLight.position;
    let light_direction: vec3<f32> = normalize(light_position_worldspace - position_worldspace);
    let light_attenuation = getSpotLightAttenuation(spotLight, position_worldspace);
    lightColor += lighting_getLightColor(
      surfaceColor,
      light_direction,
      view_direction,
      normal_worldspace,
      spotLight.color / light_attenuation
    );
  }

  for (var i: i32 = 0; i < lighting.directionalLightCount; i++) {
    let directionalLight: DirectionalLight = lighting_getDirectionalLight(i);
    lightColor += lighting_getLightColor(surfaceColor, -directionalLight.direction, view_direction, normal_worldspace, directionalLight.color);
  }  
  
  return lightColor;
}

fn lighting_getSpecularLightColor(cameraPosition: vec3<f32>, position_worldspace: vec3<f32>, normal_worldspace: vec3<f32>) -> vec3<f32>{
  var lightColor = vec3<f32>(0, 0, 0);
  let surfaceColor = vec3<f32>(0, 0, 0);

  if (lighting.enabled != 0) {
    let view_direction = normalize(cameraPosition - position_worldspace);

    for (var i: i32 = 0; i < lighting.pointLightCount; i++) {
      let pointLight: PointLight = lighting_getPointLight(i);
      let light_position_worldspace: vec3<f32> = pointLight.position;
      let light_direction: vec3<f32> = normalize(light_position_worldspace - position_worldspace);
      let light_attenuation = getPointLightAttenuation(
        pointLight,
        distance(light_position_worldspace, position_worldspace)
      );
      lightColor += lighting_getLightColor(
        surfaceColor,
        light_direction,
        view_direction,
        normal_worldspace,
        pointLight.color / light_attenuation
      );
    }

    for (var i: i32 = 0; i < lighting.spotLightCount; i++) {
      let spotLight: SpotLight = lighting_getSpotLight(i);
      let light_position_worldspace: vec3<f32> = spotLight.position;
      let light_direction: vec3<f32> = normalize(light_position_worldspace - position_worldspace);
      let light_attenuation = getSpotLightAttenuation(spotLight, position_worldspace);
      lightColor += lighting_getLightColor(
        surfaceColor,
        light_direction,
        view_direction,
        normal_worldspace,
        spotLight.color / light_attenuation
      );
    }

    for (var i: i32 = 0; i < lighting.directionalLightCount; i++) {
        let directionalLight: DirectionalLight = lighting_getDirectionalLight(i);
        lightColor += lighting_getLightColor(surfaceColor, -directionalLight.direction, view_direction, normal_worldspace, directionalLight.color);
    }
  }
  return lightColor;
}
`,Kr={props:{},name:`gouraudMaterial`,bindingLayout:[{name:`gouraudMaterial`,group:3}],vs:Wr.replace(`phongMaterial`,`gouraudMaterial`),fs:Ur.replace(`phongMaterial`,`gouraudMaterial`),source:Gr.replaceAll(`phongMaterial`,`gouraudMaterial`),defines:{LIGHTING_VERTEX:!0},dependencies:[jr,Sr],uniformTypes:{unlit:`i32`,ambient:`f32`,diffuse:`f32`,shininess:`f32`,specularColor:`vec3<f32>`},defaultUniforms:{unlit:!1,ambient:.35,diffuse:.6,shininess:32,specularColor:[38.25,38.25,38.25]},getUniforms(e){return{...Kr.defaultUniforms,...e}}},qr={name:`phongMaterial`,firstBindingSlot:0,bindingLayout:[{name:`phongMaterial`,group:3}],dependencies:[jr,Sr],source:Gr,vs:Ur,fs:Wr,defines:{LIGHTING_FRAGMENT:!0},uniformTypes:{unlit:`i32`,ambient:`f32`,diffuse:`f32`,shininess:`f32`,specularColor:`vec3<f32>`},defaultUniforms:{unlit:!1,ambient:.35,diffuse:.6,shininess:32,specularColor:[38.25,38.25,38.25]},getUniforms(e){return{...qr.defaultUniforms,...e}}},Jr=`out vec3 pbr_vPosition;
out vec2 pbr_vUV0;
out vec2 pbr_vUV1;

#ifdef HAS_NORMALS
# ifdef HAS_TANGENTS
out mat3 pbr_vTBN;
# else
out vec3 pbr_vNormal;
# endif
#endif

void pbr_setPositionNormalTangentUV(
  vec4 position,
  vec4 normal,
  vec4 tangent,
  vec2 uv0,
  vec2 uv1
)
{
  vec4 pos = pbrProjection.modelMatrix * position;
  pbr_vPosition = vec3(pos.xyz) / pos.w;

#ifdef HAS_NORMALS
#ifdef HAS_TANGENTS
  vec3 normalW = normalize(vec3(pbrProjection.normalMatrix * vec4(normal.xyz, 0.0)));
  vec3 tangentW = normalize(vec3(pbrProjection.modelMatrix * vec4(tangent.xyz, 0.0)));
  vec3 bitangentW = cross(normalW, tangentW) * tangent.w;
  pbr_vTBN = mat3(tangentW, bitangentW, normalW);
#else // HAS_TANGENTS != 1
  pbr_vNormal = normalize(vec3(pbrProjection.modelMatrix * vec4(normal.xyz, 0.0)));
#endif
#endif

#ifdef HAS_UV
  pbr_vUV0 = uv0;
#else
  pbr_vUV0 = vec2(0.,0.);
#endif

  pbr_vUV1 = uv1;
}
`,Yr=`precision highp float;

layout(std140) uniform pbrMaterialUniforms {
  // Material is unlit
  bool unlit;

  // Base color map
  bool baseColorMapEnabled;
  vec4 baseColorFactor;

  bool normalMapEnabled;  
  float normalScale; // #ifdef HAS_NORMALMAP

  bool emissiveMapEnabled;
  vec3 emissiveFactor; // #ifdef HAS_EMISSIVEMAP

  vec2 metallicRoughnessValues;
  bool metallicRoughnessMapEnabled;

  bool occlusionMapEnabled;
  float occlusionStrength; // #ifdef HAS_OCCLUSIONMAP
  
  bool alphaCutoffEnabled;
  float alphaCutoff; // #ifdef ALPHA_CUTOFF

  vec3 specularColorFactor;
  float specularIntensityFactor;
  bool specularColorMapEnabled;
  bool specularIntensityMapEnabled;

  float ior;

  float transmissionFactor;
  bool transmissionMapEnabled;

  float thicknessFactor;
  float attenuationDistance;
  vec3 attenuationColor;

  float clearcoatFactor;
  float clearcoatRoughnessFactor;
  bool clearcoatMapEnabled;
  bool clearcoatRoughnessMapEnabled;

  vec3 sheenColorFactor;
  float sheenRoughnessFactor;
  bool sheenColorMapEnabled;
  bool sheenRoughnessMapEnabled;

  float iridescenceFactor;
  float iridescenceIor;
  vec2 iridescenceThicknessRange;
  bool iridescenceMapEnabled;

  float anisotropyStrength;
  float anisotropyRotation;
  vec2 anisotropyDirection;
  bool anisotropyMapEnabled;

  float emissiveStrength;
  float dispersion;
  
  // IBL
  bool IBLenabled;
  vec2 scaleIBLAmbient; // #ifdef USE_IBL
  
  // debugging flags used for shader output of intermediate PBR variables
  // #ifdef PBR_DEBUG
  vec4 scaleDiffBaseMR;
  vec4 scaleFGDSpec;
  // #endif

  int baseColorUVSet;
  mat3 baseColorUVTransform;
  int metallicRoughnessUVSet;
  mat3 metallicRoughnessUVTransform;
  int normalUVSet;
  mat3 normalUVTransform;
  int occlusionUVSet;
  mat3 occlusionUVTransform;
  int emissiveUVSet;
  mat3 emissiveUVTransform;
  int specularColorUVSet;
  mat3 specularColorUVTransform;
  int specularIntensityUVSet;
  mat3 specularIntensityUVTransform;
  int transmissionUVSet;
  mat3 transmissionUVTransform;
  int thicknessUVSet;
  mat3 thicknessUVTransform;
  int clearcoatUVSet;
  mat3 clearcoatUVTransform;
  int clearcoatRoughnessUVSet;
  mat3 clearcoatRoughnessUVTransform;
  int clearcoatNormalUVSet;
  mat3 clearcoatNormalUVTransform;
  int sheenColorUVSet;
  mat3 sheenColorUVTransform;
  int sheenRoughnessUVSet;
  mat3 sheenRoughnessUVTransform;
  int iridescenceUVSet;
  mat3 iridescenceUVTransform;
  int iridescenceThicknessUVSet;
  mat3 iridescenceThicknessUVTransform;
  int anisotropyUVSet;
  mat3 anisotropyUVTransform;

  float bumpFactor;
  bool bumpMapEnabled;
  float diffuseTransmissionFactor;
  bool diffuseTransmissionMapEnabled;
  vec3 diffuseTransmissionColorFactor;
  bool diffuseTransmissionColorMapEnabled;
  vec3 multiscatterColorFactor;
  bool multiscatterColorMapEnabled;
  float scatterAnisotropy;

  int bumpUVSet;
  mat3 bumpUVTransform;
  int diffuseTransmissionUVSet;
  mat3 diffuseTransmissionUVTransform;
  int diffuseTransmissionColorUVSet;
  mat3 diffuseTransmissionColorUVTransform;
  int multiscatterColorUVSet;
  mat3 multiscatterColorUVTransform;
} pbrMaterial;

// Samplers
#ifdef HAS_BASECOLORMAP
uniform sampler2D pbr_baseColorSampler;
#endif
#ifdef HAS_NORMALMAP
uniform sampler2D pbr_normalSampler;
#endif
#ifdef HAS_EMISSIVEMAP
uniform sampler2D pbr_emissiveSampler;
#endif
#ifdef HAS_METALROUGHNESSMAP
uniform sampler2D pbr_metallicRoughnessSampler;
#endif
#ifdef HAS_OCCLUSIONMAP
uniform sampler2D pbr_occlusionSampler;
#endif
#ifdef HAS_SPECULARCOLORMAP
uniform sampler2D pbr_specularColorSampler;
#endif
#ifdef HAS_SPECULARINTENSITYMAP
uniform sampler2D pbr_specularIntensitySampler;
#endif
#ifdef HAS_TRANSMISSIONMAP
uniform sampler2D pbr_transmissionSampler;
#endif
#ifdef HAS_THICKNESSMAP
uniform sampler2D pbr_thicknessSampler;
#endif
#ifdef HAS_CLEARCOATMAP
uniform sampler2D pbr_clearcoatSampler;
#endif
#ifdef HAS_CLEARCOATROUGHNESSMAP
uniform sampler2D pbr_clearcoatRoughnessSampler;
#endif
#ifdef HAS_CLEARCOATNORMALMAP
uniform sampler2D pbr_clearcoatNormalSampler;
#endif
#ifdef HAS_SHEENCOLORMAP
uniform sampler2D pbr_sheenColorSampler;
#endif
#ifdef HAS_SHEENROUGHNESSMAP
uniform sampler2D pbr_sheenRoughnessSampler;
#endif
#ifdef HAS_IRIDESCENCEMAP
uniform sampler2D pbr_iridescenceSampler;
#endif
#ifdef HAS_IRIDESCENCETHICKNESSMAP
uniform sampler2D pbr_iridescenceThicknessSampler;
#endif
#ifdef HAS_ANISOTROPYMAP
uniform sampler2D pbr_anisotropySampler;
#endif
#ifdef HAS_BUMPMAP
uniform sampler2D pbr_bumpSampler;
#endif
#ifdef HAS_DIFFUSETRANSMISSIONMAP
uniform sampler2D pbr_diffuseTransmissionSampler;
#endif
#ifdef HAS_DIFFUSETRANSMISSIONCOLORMAP
uniform sampler2D pbr_diffuseTransmissionColorSampler;
#endif
#ifdef HAS_MULTISCATTERCOLORMAP
uniform sampler2D pbr_multiscatterColorSampler;
#endif
// Inputs from vertex shader

in vec3 pbr_vPosition;
in vec2 pbr_vUV0;
in vec2 pbr_vUV1;

#ifdef HAS_NORMALS
#ifdef HAS_TANGENTS
in mat3 pbr_vTBN;
#else
in vec3 pbr_vNormal;
#endif
#endif

// Encapsulate the various inputs used by the various functions in the shading equation
// We store values in this struct to simplify the integration of alternative implementations
// of the shading terms, outlined in the Readme.MD Appendix.
struct PBRInfo {
  float NdotL;                  // cos angle between normal and light direction
  float NdotV;                  // cos angle between normal and view direction
  float NdotH;                  // cos angle between normal and half vector
  float LdotH;                  // cos angle between light direction and half vector
  float VdotH;                  // cos angle between view direction and half vector
  float perceptualRoughness;    // roughness value, as authored by the model creator (input to shader)
  float metalness;              // metallic value at the surface
  vec3 reflectance0;            // full reflectance color (normal incidence angle)
  vec3 reflectance90;           // reflectance color at grazing angle
  float alphaRoughness;         // roughness mapped to a more linear change in the roughness (proposed by [2])
  vec3 diffuseColor;            // color contribution from diffuse lighting
  vec3 specularColor;           // color contribution from specular lighting
  vec3 n;                       // normal at surface point
  vec3 v;                       // vector from surface point to camera
  vec3 l;                       // direction from the surface toward the current light
  vec3 h;                       // half vector between the current light and camera
};

const float M_PI = 3.141592653589793;
const float c_MinRoughness = 0.04;

// Widen sub-pixel specular lobes using the screen-space normal footprint.
// This is geometric specular antialiasing: the normal variance is converted
// into an additional squared perceptual roughness before evaluating BRDFs.
float widenSpecularRoughness(float perceptualRoughness, vec3 normal)
{
  vec3 normalDerivativeX = dFdx(normal);
  vec3 normalDerivativeY = dFdy(normal);
  float normalVariance =
    dot(normalDerivativeX, normalDerivativeX) +
    dot(normalDerivativeY, normalDerivativeY);
  float kernelRoughnessSquared = min(2.0 * normalVariance, 1.0);
  return clamp(
    sqrt(perceptualRoughness * perceptualRoughness + kernelRoughnessSquared),
    c_MinRoughness,
    1.0
  );
}

vec3 calculateFinalColor(PBRInfo pbrInfo, vec3 lightColor);

vec4 SRGBtoLINEAR(vec4 srgbIn)
{
#ifdef MANUAL_SRGB
#ifdef SRGB_FAST_APPROXIMATION
  vec3 linOut = pow(srgbIn.xyz,vec3(2.2));
#else // SRGB_FAST_APPROXIMATION
  vec3 bLess = step(vec3(0.04045),srgbIn.xyz);
  vec3 linOut = mix( srgbIn.xyz/vec3(12.92), pow((srgbIn.xyz+vec3(0.055))/vec3(1.055),vec3(2.4)), bLess );
#endif //SRGB_FAST_APPROXIMATION
  return vec4(linOut,srgbIn.w);;
#else //MANUAL_SRGB
  return srgbIn;
#endif //MANUAL_SRGB
}

vec2 getMaterialUV(int uvSet, mat3 uvTransform)
{
  vec2 baseUV = uvSet == 1 ? pbr_vUV1 : pbr_vUV0;
  return (uvTransform * vec3(baseUV, 1.0)).xy;
}

// Build the tangent basis from interpolated attributes or screen-space derivatives.
mat3 getTBN(vec2 uv)
{
#ifndef HAS_TANGENTS
  vec3 pos_dx = dFdx(pbr_vPosition);
  vec3 pos_dy = dFdy(pbr_vPosition);
  vec3 tex_dx = dFdx(vec3(uv, 0.0));
  vec3 tex_dy = dFdy(vec3(uv, 0.0));
  vec3 t = (tex_dy.t * pos_dx - tex_dx.t * pos_dy) / (tex_dx.s * tex_dy.t - tex_dy.s * tex_dx.t);

#ifdef HAS_NORMALS
  vec3 ng = normalize(pbr_vNormal);
#else
  vec3 ng = cross(pos_dx, pos_dy);
#endif

  t = normalize(t - ng * dot(ng, t));
  vec3 b = normalize(cross(ng, t));
  mat3 tbn = mat3(t, b, ng);
#else // HAS_TANGENTS
  mat3 tbn = pbr_vTBN;
#endif

  return tbn;
}

// Find the normal for this fragment, pulling either from a predefined normal map
// or from the interpolated mesh normal and tangent attributes.
vec3 getMappedNormal(sampler2D normalSampler, mat3 tbn, float normalScale, vec2 uv)
{
  vec3 n = texture(normalSampler, uv).rgb;
  return normalize(tbn * ((2.0 * n - 1.0) * vec3(normalScale, normalScale, 1.0)));
}

vec3 getNormal(mat3 tbn, vec2 uv)
{
#ifdef HAS_NORMALMAP
  vec3 n = getMappedNormal(pbr_normalSampler, tbn, pbrMaterial.normalScale, uv);
#else
  // The tbn matrix is linearly interpolated, so we need to re-normalize
  vec3 n = normalize(tbn[2].xyz);
#endif

#ifdef HAS_BUMPMAP
  vec2 bumpUV = getMaterialUV(pbrMaterial.bumpUVSet, pbrMaterial.bumpUVTransform);
  vec2 bumpTexelSize = 1.0 / vec2(textureSize(pbr_bumpSampler, 0));
  float bumpHeight = texture(pbr_bumpSampler, bumpUV).r;
  vec2 bumpGradient = vec2(
    texture(pbr_bumpSampler, bumpUV + vec2(bumpTexelSize.x, 0.0)).r - bumpHeight,
    texture(pbr_bumpSampler, bumpUV + vec2(0.0, bumpTexelSize.y)).r - bumpHeight
  );
  n = normalize(n - pbrMaterial.bumpFactor *
    (tbn[0] * bumpGradient.x + tbn[1] * bumpGradient.y));
#endif

  return n;
}

vec3 getClearcoatNormal(mat3 tbn, vec3 baseNormal, vec2 uv)
{
#ifdef HAS_CLEARCOATNORMALMAP
  return getMappedNormal(pbr_clearcoatNormalSampler, tbn, 1.0, uv);
#else
  return baseNormal;
#endif
}

// Calculation of the lighting contribution from an optional Image Based Light source.
// Precomputed Environment Maps are required uniform inputs and are computed as outlined in [1].
// See our README.md on Environment Maps [3] for additional discussion.
#ifdef USE_IBL
vec3 getIBLContribution(PBRInfo pbrInfo, vec3 n, vec3 reflection)
{
#ifdef USE_SCENE_ENVIRONMENT
  float maximumMipLevel = max(pbrScene.environmentMipCount - 1.0, 0.0);
  float rotationSine = sin(pbrScene.environmentRotation);
  float rotationCosine = cos(pbrScene.environmentRotation);
  mat2 environmentRotation = mat2(rotationCosine, rotationSine, -rotationSine, rotationCosine);
  vec3 environmentNormal = vec3(environmentRotation * n.xz, n.y).xzy;
  vec3 environmentReflection = vec3(environmentRotation * reflection.xz, reflection.y).xzy;
#else
  float maximumMipLevel = 9.0;
  vec3 environmentNormal = n;
  vec3 environmentReflection = reflection;
#endif
  float lod = pbrInfo.perceptualRoughness * maximumMipLevel;
  // retrieve a scale and bias to F0. See [1], Figure 3
  vec4 brdfSample = texture(pbr_brdfLUT,
    vec2(pbrInfo.NdotV, 1.0 - pbrInfo.perceptualRoughness));
  vec4 diffuseSample = texture(pbr_diffuseEnvSampler, environmentNormal);

#ifdef USE_TEX_LOD
  vec4 specularSample = textureLod(pbr_specularEnvSampler, environmentReflection, lod);
#else
  vec4 specularSample = texture(pbr_specularEnvSampler, environmentReflection);
#endif

#ifdef USE_SCENE_ENVIRONMENT
  vec3 brdf = brdfSample.rgb;
  vec3 diffuseLight = diffuseSample.rgb;
  vec3 specularLight = specularSample.rgb;
#else
  vec3 brdf = SRGBtoLINEAR(brdfSample).rgb;
  vec3 diffuseLight = SRGBtoLINEAR(diffuseSample).rgb;
  vec3 specularLight = SRGBtoLINEAR(specularSample).rgb;
#endif

  vec3 diffuse = diffuseLight * pbrInfo.diffuseColor;
  vec3 specular = specularLight * (pbrInfo.specularColor * brdf.x + brdf.y);

  // For presentation, this allows us to disable IBL terms
  diffuse *= pbrMaterial.scaleIBLAmbient.x;
  specular *= pbrMaterial.scaleIBLAmbient.y;

#ifdef USE_SCENE_ENVIRONMENT
  return (diffuse + specular) * max(pbrScene.environmentIntensity, 0.0);
#else
  return diffuse + specular;
#endif
}
#endif

// Basic Lambertian diffuse
// Implementation from Lambert's Photometria https://archive.org/details/lambertsphotome00lambgoog
// See also [1], Equation 1
vec3 diffuse(PBRInfo pbrInfo)
{
  return pbrInfo.diffuseColor / M_PI;
}

// The following equation models the Fresnel reflectance term of the spec equation (aka F())
// Implementation of fresnel from [4], Equation 15
vec3 specularReflection(PBRInfo pbrInfo)
{
  return pbrInfo.reflectance0 +
    (pbrInfo.reflectance90 - pbrInfo.reflectance0) *
    pow(clamp(1.0 - pbrInfo.VdotH, 0.0, 1.0), 5.0);
}

// This calculates the specular geometric attenuation (aka G()),
// where rougher material will reflect less light back to the viewer.
// This implementation is based on [1] Equation 4, and we adopt their modifications to
// alphaRoughness as input as originally proposed in [2].
float geometricOcclusion(PBRInfo pbrInfo)
{
  float NdotL = pbrInfo.NdotL;
  float NdotV = pbrInfo.NdotV;
  float r = pbrInfo.alphaRoughness;

  float attenuationL = 2.0 * NdotL / (NdotL + sqrt(r * r + (1.0 - r * r) * (NdotL * NdotL)));
  float attenuationV = 2.0 * NdotV / (NdotV + sqrt(r * r + (1.0 - r * r) * (NdotV * NdotV)));
  return attenuationL * attenuationV;
}

// The following equation(s) model the distribution of microfacet normals across
// the area being drawn (aka D())
// Implementation from "Average Irregularity Representation of a Roughened Surface
// for Ray Reflection" by T. S. Trowbridge, and K. P. Reitz
// Follows the distribution function recommended in the SIGGRAPH 2013 course notes
// from EPIC Games [1], Equation 3.
float microfacetDistribution(PBRInfo pbrInfo)
{
  float roughnessSq = pbrInfo.alphaRoughness * pbrInfo.alphaRoughness;
  float f = (pbrInfo.NdotH * roughnessSq - pbrInfo.NdotH) * pbrInfo.NdotH + 1.0;
  return roughnessSq / (M_PI * f * f);
}

float maxComponent(vec3 value)
{
  return max(max(value.r, value.g), value.b);
}

float getDielectricF0(float ior)
{
  float clampedIor = max(ior, 1.0);
  float ratio = (clampedIor - 1.0) / (clampedIor + 1.0);
  return ratio * ratio;
}

vec2 normalizeDirection(vec2 direction)
{
  float directionLength = length(direction);
  return directionLength > 0.0001 ? direction / directionLength : vec2(1.0, 0.0);
}

vec2 rotateDirection(vec2 direction, float rotation)
{
  float s = sin(rotation);
  float c = cos(rotation);
  return vec2(direction.x * c - direction.y * s, direction.x * s + direction.y * c);
}

vec3 encodeLinearSRGB(vec3 linearColor)
{
  vec3 positiveColor = max(linearColor, vec3(0.0));
  return mix(
    positiveColor * 12.92,
    1.055 * pow(positiveColor, vec3(1.0 / 2.4)) - 0.055,
    greaterThan(positiveColor, vec3(0.0031308))
  );
}

vec3 toneMapKhronosPBRNeutral(vec3 color)
{
  const float startCompression = 0.76;
  float darkestChannel = min(color.r, min(color.g, color.b));
  float offset = darkestChannel < 0.08
    ? darkestChannel - 6.25 * darkestChannel * darkestChannel
    : 0.04;
  color -= vec3(offset);

  float peak = maxComponent(color);
  if (peak < startCompression) {
    return color;
  }

  float compressionRange = 1.0 - startCompression;
  float compressedPeak = 1.0 - compressionRange * compressionRange /
    (peak + compressionRange - startCompression);
  color *= compressedPeak / max(peak, 0.0001);
  float desaturation = 1.0 - 1.0 / (0.15 * (peak - compressedPeak) + 1.0);
  return mix(color, vec3(compressedPeak), desaturation);
}

vec3 applySceneColorManagement(vec3 sceneColor)
{
#ifdef USE_SCENE_COLOR_MANAGEMENT
  vec3 color = max(sceneColor, vec3(0.0)) * max(pbrScene.exposure, 0.0);
  if (pbrScene.toneMapMode == 1) {
    color /= vec3(1.0) + color;
  } else if (pbrScene.toneMapMode == 2) {
    color = toneMapKhronosPBRNeutral(color);
  } else if (pbrScene.toneMapMode == 3) {
    color = clamp(
      (color * (2.51 * color + 0.03)) / (color * (2.43 * color + 0.59) + 0.14),
      vec3(0.0),
      vec3(1.0)
    );
  }
  return pbrScene.outputEncoding == 0 ? color : encodeLinearSRGB(color);
#else
  return pow(max(sceneColor, vec3(0.0)), vec3(1.0 / 2.2));
#endif
}

float dielectricSchlick(float reflectance, float cosine)
{
  return reflectance + (1.0 - reflectance) * pow(clamp(1.0 - cosine, 0.0, 1.0), 5.0);
}

vec3 evaluateIridescenceSensitivity(float opticalPathDifference, vec3 phaseShift)
{
  float phase = 2.0 * M_PI * opticalPathDifference * 1.0e-9;
  vec3 sensitivity = vec3(5.4856e-13, 4.4201e-13, 5.2481e-13);
  vec3 position = vec3(1.6810e6, 1.7953e6, 2.2084e6);
  vec3 variance = vec3(4.3278e9, 9.3046e9, 6.6121e9);
  vec3 xyz = sensitivity * sqrt(2.0 * M_PI * variance) *
    cos(position * phase + phaseShift) * exp(-phase * phase * variance);
  xyz.x += 9.7470e-14 * sqrt(2.0 * M_PI * 4.5282e9) *
    cos(2.2399e6 * phase + phaseShift.x) * exp(-4.5282e9 * phase * phase);
  xyz /= 1.0685e-7;
  return mat3(
    3.2404542, -0.9692660, 0.0556434,
    -1.5371385, 1.8760108, -0.2040259,
    -0.4985314, 0.0415560, 1.0572252
  ) * xyz;
}

vec3 getIridescenceTint(float iridescence, float thickness, float NdotV, vec3 baseReflectance)
{
  if (iridescence <= 0.0 || thickness <= 0.0) {
    return baseReflectance;
  }

  float filmIor = max(pbrMaterial.iridescenceIor, 1.0);
  float sineSquared = (1.0 - NdotV * NdotV) / (filmIor * filmIor);
  float cosineSquared = 1.0 - sineSquared;
  if (cosineSquared <= 0.0) {
    return mix(baseReflectance, vec3(1.0), iridescence);
  }
  float filmCosine = sqrt(cosineSquared);
  float firstInterfaceReflectance = dielectricSchlick(getDielectricF0(filmIor), NdotV);
  float transmittedEnergy = 1.0 - firstInterfaceReflectance;

  vec3 baseIor = (vec3(1.0) + sqrt(clamp(baseReflectance, vec3(0.0), vec3(0.9999)))) /
    (vec3(1.0) - sqrt(clamp(baseReflectance, vec3(0.0), vec3(0.9999))));
  vec3 secondInterfaceF0 = (baseIor - vec3(filmIor)) / (baseIor + vec3(filmIor));
  secondInterfaceF0 *= secondInterfaceF0;
  vec3 secondInterfaceReflectance = secondInterfaceF0 +
    (vec3(1.0) - secondInterfaceF0) * pow(1.0 - filmCosine, 5.0);
  vec3 phaseShift = vec3(M_PI);
  phaseShift += mix(vec3(0.0), vec3(M_PI), lessThan(baseIor, vec3(filmIor)));
  float opticalPathDifference = 2.0 * filmIor * thickness * filmCosine;
  vec3 combinedReflectance = clamp(
    firstInterfaceReflectance * secondInterfaceReflectance,
    vec3(0.00001),
    vec3(0.9999)
  );
  vec3 recurringAmplitude = sqrt(combinedReflectance);
  vec3 interfaceResponse = transmittedEnergy * transmittedEnergy * secondInterfaceReflectance /
    (vec3(1.0) - combinedReflectance);
  vec3 reflectedSpectrum = vec3(firstInterfaceReflectance) + interfaceResponse;
  vec3 harmonicAmplitude = interfaceResponse - vec3(transmittedEnergy);
  for (int harmonic = 1; harmonic <= 2; harmonic++) {
    harmonicAmplitude *= recurringAmplitude;
    reflectedSpectrum += harmonicAmplitude * 2.0 * evaluateIridescenceSensitivity(
      float(harmonic) * opticalPathDifference,
      float(harmonic) * phaseShift
    );
  }
  return mix(baseReflectance, clamp(reflectedSpectrum, vec3(0.0), vec3(1.0)), iridescence);
}

vec3 getVolumeAttenuation(float thickness)
{
  if (thickness <= 0.0) {
    return vec3(1.0);
  }

  vec3 attenuationCoefficient =
    -log(max(pbrMaterial.attenuationColor, vec3(0.0001))) /
    max(pbrMaterial.attenuationDistance, 0.0001);
  return exp(-attenuationCoefficient * thickness);
}

// KHR_materials_volume_scatter is an active draft. This evaluates a local,
// thickness-aware single-scattering approximation rather than random walk.
vec3 getDiffuseTransmissionAttenuation(
  PBRInfo pbrInfo,
  vec3 multiscatterColor,
  float thickness
)
{
  vec3 volumeAttenuation = getVolumeAttenuation(thickness);
  float scatteringStrength = maxComponent(multiscatterColor);
  if (thickness <= 0.0 || scatteringStrength <= 0.0001) {
    return volumeAttenuation;
  }

  float anisotropy = clamp(pbrMaterial.scatterAnisotropy, -0.95, 0.95);
  float scatteringCosine = clamp(dot(-pbrInfo.v, pbrInfo.l), -1.0, 1.0);
  float phaseDenominator = max(
    1.0 + anisotropy * anisotropy - 2.0 * anisotropy * scatteringCosine,
    0.0001
  );
  float phaseWeight = clamp(
    (1.0 - anisotropy * anisotropy) / pow(phaseDenominator, 1.5),
    0.0,
    4.0
  );
  float scatteringDepth = thickness / max(pbrMaterial.attenuationDistance, 0.0001);
  float scatteringProbability = 1.0 - exp(-scatteringDepth);
  vec3 scatteringColor = clamp(multiscatterColor, vec3(0.0), vec3(1.0));
  return mix(
    volumeAttenuation,
    volumeAttenuation * mix(vec3(1.0), scatteringColor * phaseWeight, scatteringColor),
    scatteringProbability
  );
}

vec3 calculateDiffuseTransmissionLight(
  PBRInfo pbrInfo,
  vec3 lightColor,
  vec3 diffuseTransmissionColor,
  float diffuseTransmission,
  vec3 multiscatterColor,
  float thickness
)
{
  float oppositeHemisphere = max(dot(-pbrInfo.n, pbrInfo.l), 0.0);
  if (oppositeHemisphere <= 0.0 || diffuseTransmission <= 0.0) {
    return vec3(0.0);
  }

  vec3 nonReflectedEnergy = vec3(1.0) - clamp(pbrInfo.reflectance0, vec3(0.0), vec3(1.0));
  vec3 attenuatedColor = getDiffuseTransmissionAttenuation(
    pbrInfo,
    multiscatterColor,
    thickness
  );
  return lightColor * diffuseTransmissionColor * nonReflectedEnergy *
    attenuatedColor * (diffuseTransmission * oppositeHemisphere / M_PI);
}

#ifdef USE_IBL
vec3 calculateDiffuseTransmissionIBL(
  PBRInfo pbrInfo,
  vec3 diffuseTransmissionColor,
  float diffuseTransmission,
  vec3 multiscatterColor,
  float thickness
)
{
  if (diffuseTransmission <= 0.0) {
    return vec3(0.0);
  }

#ifdef USE_SCENE_ENVIRONMENT
  float rotationSine = sin(pbrScene.environmentRotation);
  float rotationCosine = cos(pbrScene.environmentRotation);
  mat2 environmentRotation = mat2(rotationCosine, rotationSine, -rotationSine, rotationCosine);
  vec3 oppositeNormal = vec3(environmentRotation * -pbrInfo.n.xz, -pbrInfo.n.y).xzy;
  vec3 environmentColor = texture(pbr_diffuseEnvSampler, oppositeNormal).rgb *
    max(pbrScene.environmentIntensity, 0.0);
#else
  vec3 environmentColor = SRGBtoLINEAR(texture(pbr_diffuseEnvSampler, -pbrInfo.n)).rgb;
#endif
  vec3 nonReflectedEnergy = vec3(1.0) - clamp(pbrInfo.reflectance0, vec3(0.0), vec3(1.0));
  return environmentColor * diffuseTransmissionColor * nonReflectedEnergy *
    getDiffuseTransmissionAttenuation(pbrInfo, multiscatterColor, thickness) *
    diffuseTransmission * pbrMaterial.scaleIBLAmbient.x;
}
#endif

#ifdef USE_TRANSMISSION_FRAMEBUFFER
vec3 sampleTransmittedSceneColor(
  vec3 position,
  vec3 normal,
  vec3 viewDirection,
  float thickness,
  float perceptualRoughness,
  float indexOfRefraction
)
{
  vec3 refractionDirection = refract(
    -viewDirection,
    normal,
    1.0 / max(indexOfRefraction, 1.0)
  );
  vec3 refractedPosition = position + refractionDirection * thickness;
  vec4 clipPosition = pbrScene.projectionMatrix *
    pbrScene.viewMatrix * vec4(refractedPosition, 1.0);
  vec2 textureCoordinate = clipPosition.xy / max(clipPosition.w, 0.0001) * 0.5 + 0.5;
  textureCoordinate = clamp(textureCoordinate, vec2(0.001), vec2(0.999));

  vec2 blurRadius = perceptualRoughness * perceptualRoughness * 8.0 /
    max(pbrScene.framebufferSize, vec2(1.0));
  vec3 sceneColor = texture(pbr_transmissionFramebufferSampler, textureCoordinate).rgb * 0.4;
  sceneColor += texture(
    pbr_transmissionFramebufferSampler,
    textureCoordinate + vec2(blurRadius.x, 0.0)
  ).rgb * 0.15;
  sceneColor += texture(
    pbr_transmissionFramebufferSampler,
    textureCoordinate - vec2(blurRadius.x, 0.0)
  ).rgb * 0.15;
  sceneColor += texture(
    pbr_transmissionFramebufferSampler,
    textureCoordinate + vec2(0.0, blurRadius.y)
  ).rgb * 0.15;
  sceneColor += texture(
    pbr_transmissionFramebufferSampler,
    textureCoordinate - vec2(0.0, blurRadius.y)
  ).rgb * 0.15;
  return max(sceneColor, vec3(0.0));
}

vec3 getTransmittedSceneColor(
  vec3 position,
  vec3 normal,
  vec3 viewDirection,
  float thickness,
  float perceptualRoughness
)
{
  if (pbrMaterial.dispersion <= 0.0) {
    return sampleTransmittedSceneColor(
      position,
      normal,
      viewDirection,
      thickness,
      perceptualRoughness,
      pbrMaterial.ior
    );
  }

  float halfSpread = (max(pbrMaterial.ior, 1.0) - 1.0) * 0.025 * pbrMaterial.dispersion;
  vec3 indicesOfRefraction = max(
    vec3(pbrMaterial.ior - halfSpread, pbrMaterial.ior, pbrMaterial.ior + halfSpread),
    vec3(1.0)
  );
  return vec3(
    sampleTransmittedSceneColor(
      position, normal, viewDirection, thickness, perceptualRoughness, indicesOfRefraction.r
    ).r,
    sampleTransmittedSceneColor(
      position, normal, viewDirection, thickness, perceptualRoughness, indicesOfRefraction.g
    ).g,
    sampleTransmittedSceneColor(
      position, normal, viewDirection, thickness, perceptualRoughness, indicesOfRefraction.b
    ).b
  );
}
#endif

PBRInfo createClearcoatPBRInfo(PBRInfo basePBRInfo, vec3 clearcoatNormal, float clearcoatRoughness)
{
  float perceptualRoughness = clamp(clearcoatRoughness, c_MinRoughness, 1.0);
  float alphaRoughness = perceptualRoughness * perceptualRoughness;
  float NdotV = clamp(abs(dot(clearcoatNormal, basePBRInfo.v)), 0.001, 1.0);

  return PBRInfo(
    basePBRInfo.NdotL,
    NdotV,
    basePBRInfo.NdotH,
    basePBRInfo.LdotH,
    basePBRInfo.VdotH,
    perceptualRoughness,
    0.0,
    vec3(0.04),
    vec3(1.0),
    alphaRoughness,
    vec3(0.0),
    vec3(0.04),
    clearcoatNormal,
    basePBRInfo.v,
    basePBRInfo.l,
    basePBRInfo.h
  );
}

vec3 calculateClearcoatContribution(
  PBRInfo pbrInfo,
  vec3 lightColor,
  vec3 clearcoatNormal,
  float clearcoatFactor,
  float clearcoatRoughness
) {
  if (clearcoatFactor <= 0.0) {
    return vec3(0.0);
  }

  PBRInfo clearcoatPBRInfo = createClearcoatPBRInfo(pbrInfo, clearcoatNormal, clearcoatRoughness);
  return calculateFinalColor(clearcoatPBRInfo, lightColor) * clearcoatFactor;
}

#ifdef USE_IBL
vec3 calculateClearcoatIBLContribution(
  PBRInfo pbrInfo,
  vec3 clearcoatNormal,
  vec3 reflection,
  float clearcoatFactor,
  float clearcoatRoughness
) {
  if (clearcoatFactor <= 0.0) {
    return vec3(0.0);
  }

  PBRInfo clearcoatPBRInfo = createClearcoatPBRInfo(pbrInfo, clearcoatNormal, clearcoatRoughness);
  return getIBLContribution(clearcoatPBRInfo, clearcoatNormal, reflection) * clearcoatFactor;
}
#endif

vec3 calculateSheenContribution(
  PBRInfo pbrInfo,
  vec3 lightColor,
  vec3 sheenColor,
  float sheenRoughness
) {
  if (maxComponent(sheenColor) <= 0.0) {
    return vec3(0.0);
  }

  float alpha = max(sheenRoughness * sheenRoughness, 0.0001);
  float inverseAlpha = 1.0 / alpha;
  float sineSquared = max(1.0 - pbrInfo.NdotH * pbrInfo.NdotH, 0.0);
  float distribution = (2.0 + inverseAlpha) * pow(sineSquared, inverseAlpha * 0.5) /
    (2.0 * M_PI);
  float visibility = 1.0 / max(
    4.0 * (pbrInfo.NdotL + pbrInfo.NdotV - pbrInfo.NdotL * pbrInfo.NdotV),
    0.0001
  );
  return pbrInfo.NdotL * lightColor * sheenColor * distribution * visibility *
    (1.0 - pbrInfo.metalness);
}

vec3 calculateAnisotropicLightColor(
  PBRInfo pbrInfo,
  vec3 lightColor,
  vec3 anisotropyTangent,
  float anisotropyStrength
) {
  if (anisotropyStrength <= 0.0) {
    return calculateFinalColor(pbrInfo, lightColor);
  }

  vec3 anisotropyBitangent = normalize(cross(pbrInfo.n, anisotropyTangent));
  float tangentRoughness = mix(
    pbrInfo.alphaRoughness,
    1.0,
    anisotropyStrength * anisotropyStrength
  );
  float bitangentRoughness = clamp(pbrInfo.alphaRoughness, 0.001, 1.0);
  float roughnessProduct = tangentRoughness * bitangentRoughness;
  vec3 distributionVector = vec3(
    bitangentRoughness * dot(anisotropyTangent, pbrInfo.h),
    tangentRoughness * dot(anisotropyBitangent, pbrInfo.h),
    roughnessProduct * pbrInfo.NdotH
  );
  float distributionFactor = roughnessProduct /
    max(dot(distributionVector, distributionVector), 0.000001);
  float distribution = roughnessProduct * distributionFactor * distributionFactor / M_PI;
  float viewMask = pbrInfo.NdotL * length(vec3(
    tangentRoughness * dot(anisotropyTangent, pbrInfo.v),
    bitangentRoughness * dot(anisotropyBitangent, pbrInfo.v),
    pbrInfo.NdotV
  ));
  float lightMask = pbrInfo.NdotV * length(vec3(
    tangentRoughness * dot(anisotropyTangent, pbrInfo.l),
    bitangentRoughness * dot(anisotropyBitangent, pbrInfo.l),
    pbrInfo.NdotL
  ));
  float visibility = clamp(0.5 / max(viewMask + lightMask, 0.000001), 0.0, 1.0);
  vec3 fresnel = specularReflection(pbrInfo);
  vec3 diffuseContribution = (vec3(1.0) - fresnel) * diffuse(pbrInfo);
  return pbrInfo.NdotL * lightColor *
    (diffuseContribution + fresnel * distribution * visibility);
}

vec3 getAnisotropicReflection(PBRInfo pbrInfo, vec3 anisotropyTangent, float anisotropyStrength)
{
  if (anisotropyStrength <= 0.0) {
    return -normalize(reflect(pbrInfo.v, pbrInfo.n));
  }
  vec3 anisotropyBitangent = normalize(cross(pbrInfo.n, anisotropyTangent));
  vec3 anisotropicNormal = normalize(cross(anisotropyBitangent, pbrInfo.v));
  anisotropicNormal = normalize(cross(anisotropicNormal, anisotropyBitangent));
  float bend = anisotropyStrength * (1.0 - pbrInfo.perceptualRoughness);
  return -normalize(reflect(pbrInfo.v, normalize(mix(pbrInfo.n, anisotropicNormal, bend))));
}

vec3 calculateMaterialLightColor(
  PBRInfo pbrInfo,
  vec3 lightColor,
  vec3 clearcoatNormal,
  float clearcoatFactor,
  float clearcoatRoughness,
  vec3 sheenColor,
  float sheenRoughness,
  vec3 anisotropyTangent,
  float anisotropyStrength
) {
  vec3 color = calculateAnisotropicLightColor(
    pbrInfo,
    lightColor,
    anisotropyTangent,
    anisotropyStrength
  );
  color += calculateClearcoatContribution(
    pbrInfo,
    lightColor,
    clearcoatNormal,
    clearcoatFactor,
    clearcoatRoughness
  );
  color += calculateSheenContribution(pbrInfo, lightColor, sheenColor, sheenRoughness);
  return color;
}

void PBRInfo_setAmbientLight(inout PBRInfo pbrInfo) {
  pbrInfo.NdotL = 1.0;
  pbrInfo.NdotH = 0.0;
  pbrInfo.LdotH = 0.0;
  pbrInfo.VdotH = 1.0;
  pbrInfo.l = pbrInfo.n;
  pbrInfo.h = pbrInfo.n;
}

void PBRInfo_setDirectionalLight(inout PBRInfo pbrInfo, vec3 lightDirection) {
  vec3 n = pbrInfo.n;
  vec3 v = pbrInfo.v;
  vec3 l = normalize(lightDirection);             // Vector from surface point to light
  vec3 h = normalize(l+v);                        // Half vector between both l and v

  pbrInfo.NdotL = clamp(dot(n, l), 0.001, 1.0);
  pbrInfo.NdotH = clamp(dot(n, h), 0.0, 1.0);
  pbrInfo.LdotH = clamp(dot(l, h), 0.0, 1.0);
  pbrInfo.VdotH = clamp(dot(v, h), 0.0, 1.0);
  pbrInfo.l = l;
  pbrInfo.h = h;
}

void PBRInfo_setPointLight(inout PBRInfo pbrInfo, PointLight pointLight) {
  vec3 light_direction = normalize(pointLight.position - pbr_vPosition);
  PBRInfo_setDirectionalLight(pbrInfo, light_direction);
}

void PBRInfo_setSpotLight(inout PBRInfo pbrInfo, SpotLight spotLight) {
  vec3 light_direction = normalize(spotLight.position - pbr_vPosition);
  PBRInfo_setDirectionalLight(pbrInfo, light_direction);
}

vec3 calculateFinalColor(PBRInfo pbrInfo, vec3 lightColor) {
  // Calculate the shading terms for the microfacet specular shading model
  vec3 F = specularReflection(pbrInfo);
  float G = geometricOcclusion(pbrInfo);
  float D = microfacetDistribution(pbrInfo);

  // Calculation of analytical lighting contribution
  vec3 diffuseContrib = (1.0 - F) * diffuse(pbrInfo);
  vec3 specContrib = F * G * D / (4.0 * pbrInfo.NdotL * pbrInfo.NdotV);
  // Obtain final intensity as reflectance (BRDF) scaled by the energy of the light (cosine law)
  return pbrInfo.NdotL * lightColor * (diffuseContrib + specContrib);
}

vec4 pbr_filterColor(vec4 vertexColor)
{
  vec2 baseColorUV = getMaterialUV(pbrMaterial.baseColorUVSet, pbrMaterial.baseColorUVTransform);
  vec2 metallicRoughnessUV = getMaterialUV(
    pbrMaterial.metallicRoughnessUVSet,
    pbrMaterial.metallicRoughnessUVTransform
  );
  vec2 normalUV = getMaterialUV(pbrMaterial.normalUVSet, pbrMaterial.normalUVTransform);
  vec2 occlusionUV = getMaterialUV(pbrMaterial.occlusionUVSet, pbrMaterial.occlusionUVTransform);
  vec2 emissiveUV = getMaterialUV(pbrMaterial.emissiveUVSet, pbrMaterial.emissiveUVTransform);
  vec2 specularColorUV = getMaterialUV(
    pbrMaterial.specularColorUVSet,
    pbrMaterial.specularColorUVTransform
  );
  vec2 specularIntensityUV = getMaterialUV(
    pbrMaterial.specularIntensityUVSet,
    pbrMaterial.specularIntensityUVTransform
  );
  vec2 transmissionUV = getMaterialUV(
    pbrMaterial.transmissionUVSet,
    pbrMaterial.transmissionUVTransform
  );
  vec2 thicknessUV = getMaterialUV(pbrMaterial.thicknessUVSet, pbrMaterial.thicknessUVTransform);
  vec2 clearcoatUV = getMaterialUV(pbrMaterial.clearcoatUVSet, pbrMaterial.clearcoatUVTransform);
  vec2 clearcoatRoughnessUV = getMaterialUV(
    pbrMaterial.clearcoatRoughnessUVSet,
    pbrMaterial.clearcoatRoughnessUVTransform
  );
  vec2 clearcoatNormalUV = getMaterialUV(
    pbrMaterial.clearcoatNormalUVSet,
    pbrMaterial.clearcoatNormalUVTransform
  );
  vec2 sheenColorUV = getMaterialUV(
    pbrMaterial.sheenColorUVSet,
    pbrMaterial.sheenColorUVTransform
  );
  vec2 sheenRoughnessUV = getMaterialUV(
    pbrMaterial.sheenRoughnessUVSet,
    pbrMaterial.sheenRoughnessUVTransform
  );
  vec2 iridescenceUV = getMaterialUV(
    pbrMaterial.iridescenceUVSet,
    pbrMaterial.iridescenceUVTransform
  );
  vec2 iridescenceThicknessUV = getMaterialUV(
    pbrMaterial.iridescenceThicknessUVSet,
    pbrMaterial.iridescenceThicknessUVTransform
  );
  vec2 anisotropyUV = getMaterialUV(
    pbrMaterial.anisotropyUVSet,
    pbrMaterial.anisotropyUVTransform
  );
  vec2 diffuseTransmissionUV = getMaterialUV(
    pbrMaterial.diffuseTransmissionUVSet,
    pbrMaterial.diffuseTransmissionUVTransform
  );
  vec2 diffuseTransmissionColorUV = getMaterialUV(
    pbrMaterial.diffuseTransmissionColorUVSet,
    pbrMaterial.diffuseTransmissionColorUVTransform
  );
  vec2 multiscatterColorUV = getMaterialUV(
    pbrMaterial.multiscatterColorUVSet,
    pbrMaterial.multiscatterColorUVTransform
  );

  // The albedo may be defined from a base texture or a flat color
#ifdef HAS_BASECOLORMAP
  vec4 baseColor =
    SRGBtoLINEAR(texture(pbr_baseColorSampler, baseColorUV)) *
    pbrMaterial.baseColorFactor * vertexColor;
#else
  vec4 baseColor = pbrMaterial.baseColorFactor * vertexColor;
#endif

#ifdef ALPHA_CUTOFF
  if (baseColor.a < pbrMaterial.alphaCutoff) {
    discard;
  }
#endif

  vec3 color = vec3(0, 0, 0);

  float transmission = 0.0;

  if(pbrMaterial.unlit){
    color.rgb = baseColor.rgb;
  }
  else{
    // Metallic and Roughness material properties are packed together
    // In glTF, these factors can be specified by fixed scalar values
    // or from a metallic-roughness map
    float perceptualRoughness = pbrMaterial.metallicRoughnessValues.y;
    float metallic = pbrMaterial.metallicRoughnessValues.x;
#ifdef HAS_METALROUGHNESSMAP
    // Roughness is stored in the 'g' channel, metallic is stored in the 'b' channel.
    // This layout intentionally reserves the 'r' channel for (optional) occlusion map data
    vec4 mrSample = texture(pbr_metallicRoughnessSampler, metallicRoughnessUV);
    perceptualRoughness = mrSample.g * perceptualRoughness;
    metallic = mrSample.b * metallic;
#endif
    perceptualRoughness = clamp(perceptualRoughness, c_MinRoughness, 1.0);
    metallic = clamp(metallic, 0.0, 1.0);
    mat3 tbn = getTBN(normalUV);
    vec3 n = getNormal(tbn, normalUV);                          // normal at surface point
    perceptualRoughness = widenSpecularRoughness(perceptualRoughness, n);
    vec3 v = normalize(pbrProjection.camera - pbr_vPosition);  // Vector from surface point to camera
    float NdotV = clamp(abs(dot(n, v)), 0.001, 1.0);
#ifdef USE_MATERIAL_EXTENSIONS
    bool useExtendedPBR =
      pbrMaterial.specularColorMapEnabled ||
      pbrMaterial.specularIntensityMapEnabled ||
      abs(pbrMaterial.specularIntensityFactor - 1.0) > 0.0001 ||
      maxComponent(abs(pbrMaterial.specularColorFactor - vec3(1.0))) > 0.0001 ||
      abs(pbrMaterial.ior - 1.5) > 0.0001 ||
      pbrMaterial.dispersion > 0.0001 ||
      pbrMaterial.transmissionMapEnabled ||
      pbrMaterial.transmissionFactor > 0.0001 ||
      pbrMaterial.diffuseTransmissionMapEnabled ||
      pbrMaterial.diffuseTransmissionColorMapEnabled ||
      pbrMaterial.diffuseTransmissionFactor > 0.0001 ||
      pbrMaterial.multiscatterColorMapEnabled ||
      maxComponent(pbrMaterial.multiscatterColorFactor) > 0.0001 ||
      pbrMaterial.clearcoatMapEnabled ||
      pbrMaterial.clearcoatRoughnessMapEnabled ||
      pbrMaterial.clearcoatFactor > 0.0001 ||
      pbrMaterial.clearcoatRoughnessFactor > 0.0001 ||
      pbrMaterial.sheenColorMapEnabled ||
      pbrMaterial.sheenRoughnessMapEnabled ||
      maxComponent(pbrMaterial.sheenColorFactor) > 0.0001 ||
      pbrMaterial.sheenRoughnessFactor > 0.0001 ||
      pbrMaterial.iridescenceMapEnabled ||
      pbrMaterial.iridescenceFactor > 0.0001 ||
      abs(pbrMaterial.iridescenceIor - 1.3) > 0.0001 ||
      abs(pbrMaterial.iridescenceThicknessRange.x - 100.0) > 0.0001 ||
      abs(pbrMaterial.iridescenceThicknessRange.y - 400.0) > 0.0001 ||
      pbrMaterial.anisotropyMapEnabled ||
      pbrMaterial.anisotropyStrength > 0.0001 ||
      abs(pbrMaterial.anisotropyRotation) > 0.0001 ||
      length(pbrMaterial.anisotropyDirection - vec2(1.0, 0.0)) > 0.0001;
#else
    bool useExtendedPBR = false;
#endif

    if (!useExtendedPBR) {
      // Keep the baseline metallic-roughness implementation byte-for-byte equivalent in behavior.
      float alphaRoughness = perceptualRoughness * perceptualRoughness;

      vec3 f0 = vec3(0.04);
      vec3 diffuseColor = baseColor.rgb * (vec3(1.0) - f0);
      diffuseColor *= 1.0 - metallic;
      vec3 specularColor = mix(f0, baseColor.rgb, metallic);

      float reflectance = max(max(specularColor.r, specularColor.g), specularColor.b);
      float reflectance90 = clamp(reflectance * 25.0, 0.0, 1.0);
      vec3 specularEnvironmentR0 = specularColor.rgb;
      vec3 specularEnvironmentR90 = vec3(1.0, 1.0, 1.0) * reflectance90;
      vec3 reflection = -normalize(reflect(v, n));

      PBRInfo pbrInfo = PBRInfo(
        0.0, // NdotL
        NdotV,
        0.0, // NdotH
        0.0, // LdotH
        0.0, // VdotH
        perceptualRoughness,
        metallic,
        specularEnvironmentR0,
        specularEnvironmentR90,
        alphaRoughness,
        diffuseColor,
        specularColor,
        n,
        v,
        n,
        n
      );

#ifdef USE_LIGHTS
      PBRInfo_setAmbientLight(pbrInfo);
      color += calculateFinalColor(pbrInfo, lighting.ambientColor);

      for(int i = 0; i < lighting.directionalLightCount; i++) {
        if (i < lighting.directionalLightCount) {
          PBRInfo_setDirectionalLight(pbrInfo, lighting_getDirectionalLight(i).direction);
          color += calculateFinalColor(pbrInfo, lighting_getDirectionalLight(i).color);
        }
      }

      for(int i = 0; i < lighting.pointLightCount; i++) {
        if (i < lighting.pointLightCount) {
          PBRInfo_setPointLight(pbrInfo, lighting_getPointLight(i));
          float attenuation = getPointLightAttenuation(lighting_getPointLight(i), distance(lighting_getPointLight(i).position, pbr_vPosition));
          color += calculateFinalColor(pbrInfo, lighting_getPointLight(i).color / attenuation);
        }
      }

      for(int i = 0; i < lighting.spotLightCount; i++) {
        if (i < lighting.spotLightCount) {
          PBRInfo_setSpotLight(pbrInfo, lighting_getSpotLight(i));
          float attenuation = getSpotLightAttenuation(lighting_getSpotLight(i), pbr_vPosition);
          color += calculateFinalColor(pbrInfo, lighting_getSpotLight(i).color / attenuation);
        }
      }
#endif

#ifdef USE_IBL
      if (pbrMaterial.IBLenabled) {
        color += getIBLContribution(pbrInfo, n, reflection);
      }
#endif

#ifdef HAS_OCCLUSIONMAP
      if (pbrMaterial.occlusionMapEnabled) {
        float ao = texture(pbr_occlusionSampler, occlusionUV).r;
        color = mix(color, color * ao, pbrMaterial.occlusionStrength);
      }
#endif

      vec3 emissive = pbrMaterial.emissiveFactor;
#ifdef HAS_EMISSIVEMAP
      if (pbrMaterial.emissiveMapEnabled) {
        emissive *= SRGBtoLINEAR(texture(pbr_emissiveSampler, emissiveUV)).rgb;
      }
#endif
      color += emissive * pbrMaterial.emissiveStrength;

#ifdef PBR_DEBUG
      color = mix(color, baseColor.rgb, pbrMaterial.scaleDiffBaseMR.y);
      color = mix(color, vec3(metallic), pbrMaterial.scaleDiffBaseMR.z);
      color = mix(color, vec3(perceptualRoughness), pbrMaterial.scaleDiffBaseMR.w);
#endif

      return vec4(applySceneColorManagement(color), baseColor.a);
    }

    float specularIntensity = pbrMaterial.specularIntensityFactor;
#ifdef HAS_SPECULARINTENSITYMAP
    if (pbrMaterial.specularIntensityMapEnabled) {
      specularIntensity *= texture(pbr_specularIntensitySampler, specularIntensityUV).a;
    }
#endif

    vec3 specularFactor = pbrMaterial.specularColorFactor;
#ifdef HAS_SPECULARCOLORMAP
    if (pbrMaterial.specularColorMapEnabled) {
      specularFactor *= SRGBtoLINEAR(texture(pbr_specularColorSampler, specularColorUV)).rgb;
    }
#endif

    transmission = pbrMaterial.transmissionFactor;
#ifdef HAS_TRANSMISSIONMAP
    if (pbrMaterial.transmissionMapEnabled) {
      transmission *= texture(pbr_transmissionSampler, transmissionUV).r;
    }
#endif
    transmission = clamp(transmission * (1.0 - metallic), 0.0, 1.0);
    float thickness = max(pbrMaterial.thicknessFactor, 0.0);
#ifdef HAS_THICKNESSMAP
    thickness *= texture(pbr_thicknessSampler, thicknessUV).g;
#endif

    float diffuseTransmission = clamp(pbrMaterial.diffuseTransmissionFactor, 0.0, 1.0);
#ifdef HAS_DIFFUSETRANSMISSIONMAP
    if (pbrMaterial.diffuseTransmissionMapEnabled) {
      diffuseTransmission *= texture(pbr_diffuseTransmissionSampler, diffuseTransmissionUV).a;
    }
#endif
    diffuseTransmission *= (1.0 - metallic) * (1.0 - transmission);
    vec3 diffuseTransmissionColor = pbrMaterial.diffuseTransmissionColorFactor;
#ifdef HAS_DIFFUSETRANSMISSIONCOLORMAP
    if (pbrMaterial.diffuseTransmissionColorMapEnabled) {
      diffuseTransmissionColor *= SRGBtoLINEAR(
        texture(pbr_diffuseTransmissionColorSampler, diffuseTransmissionColorUV)
      ).rgb;
    }
#endif
    vec3 multiscatterColor = pbrMaterial.multiscatterColorFactor;
#ifdef HAS_MULTISCATTERCOLORMAP
    if (pbrMaterial.multiscatterColorMapEnabled) {
      multiscatterColor *= SRGBtoLINEAR(
        texture(pbr_multiscatterColorSampler, multiscatterColorUV)
      ).rgb;
    }
#endif

    float clearcoatFactor = pbrMaterial.clearcoatFactor;
    float clearcoatRoughness = pbrMaterial.clearcoatRoughnessFactor;
#ifdef HAS_CLEARCOATMAP
    if (pbrMaterial.clearcoatMapEnabled) {
      clearcoatFactor *= texture(pbr_clearcoatSampler, clearcoatUV).r;
    }
#endif
#ifdef HAS_CLEARCOATROUGHNESSMAP
    if (pbrMaterial.clearcoatRoughnessMapEnabled) {
      clearcoatRoughness *= texture(pbr_clearcoatRoughnessSampler, clearcoatRoughnessUV).g;
    }
#endif
    clearcoatFactor = clamp(clearcoatFactor, 0.0, 1.0);
    clearcoatRoughness = clamp(clearcoatRoughness, c_MinRoughness, 1.0);
    vec3 clearcoatNormal = getClearcoatNormal(getTBN(clearcoatNormalUV), n, clearcoatNormalUV);
    clearcoatRoughness = widenSpecularRoughness(clearcoatRoughness, clearcoatNormal);

    vec3 sheenColor = pbrMaterial.sheenColorFactor;
    float sheenRoughness = pbrMaterial.sheenRoughnessFactor;
#ifdef HAS_SHEENCOLORMAP
    if (pbrMaterial.sheenColorMapEnabled) {
      sheenColor *= SRGBtoLINEAR(texture(pbr_sheenColorSampler, sheenColorUV)).rgb;
    }
#endif
#ifdef HAS_SHEENROUGHNESSMAP
    if (pbrMaterial.sheenRoughnessMapEnabled) {
      sheenRoughness *= texture(pbr_sheenRoughnessSampler, sheenRoughnessUV).a;
    }
#endif
    sheenRoughness = clamp(sheenRoughness, c_MinRoughness, 1.0);

    float iridescence = pbrMaterial.iridescenceFactor;
#ifdef HAS_IRIDESCENCEMAP
    if (pbrMaterial.iridescenceMapEnabled) {
      iridescence *= texture(pbr_iridescenceSampler, iridescenceUV).r;
    }
#endif
    iridescence = clamp(iridescence, 0.0, 1.0);
    float iridescenceThickness = mix(
      pbrMaterial.iridescenceThicknessRange.x,
      pbrMaterial.iridescenceThicknessRange.y,
      0.5
    );
#ifdef HAS_IRIDESCENCETHICKNESSMAP
    iridescenceThickness = mix(
      pbrMaterial.iridescenceThicknessRange.x,
      pbrMaterial.iridescenceThicknessRange.y,
      texture(pbr_iridescenceThicknessSampler, iridescenceThicknessUV).g
    );
#endif

    float anisotropyStrength = clamp(pbrMaterial.anisotropyStrength, 0.0, 1.0);
    vec2 anisotropyDirection = normalizeDirection(pbrMaterial.anisotropyDirection);
#ifdef HAS_ANISOTROPYMAP
    if (pbrMaterial.anisotropyMapEnabled) {
      vec3 anisotropySample = texture(pbr_anisotropySampler, anisotropyUV).rgb;
      anisotropyStrength *= anisotropySample.b;
      vec2 mappedDirection = anisotropySample.rg * 2.0 - 1.0;
      if (length(mappedDirection) > 0.0001) {
        anisotropyDirection = normalize(mappedDirection);
      }
    }
#endif
    anisotropyDirection = rotateDirection(anisotropyDirection, pbrMaterial.anisotropyRotation);
    vec3 anisotropyTangent = normalize(tbn[0] * anisotropyDirection.x + tbn[1] * anisotropyDirection.y);
    if (length(anisotropyTangent) < 0.0001) {
      anisotropyTangent = normalize(tbn[0]);
    }
    // Roughness is authored as perceptual roughness; as is convention,
    // convert to material roughness by squaring the perceptual roughness [2].
    float alphaRoughness = perceptualRoughness * perceptualRoughness;

    float dielectricF0 = getDielectricF0(pbrMaterial.ior);
    vec3 dielectricSpecularF0 = min(
      vec3(dielectricF0) * specularFactor * specularIntensity,
      vec3(1.0)
    );
    dielectricSpecularF0 = getIridescenceTint(
      iridescence,
      iridescenceThickness,
      NdotV,
      dielectricSpecularF0
    );
    vec3 diffuseColor = baseColor.rgb * (vec3(1.0) - dielectricSpecularF0);
    diffuseColor *= (1.0 - metallic) * (1.0 - transmission) * (1.0 - diffuseTransmission);
    vec3 specularColor = mix(dielectricSpecularF0, baseColor.rgb, metallic);

    float clearcoatViewFresnel = dielectricSchlick(
      0.04,
      clamp(abs(dot(clearcoatNormal, v)), 0.0, 1.0)
    );
    float sheenDirectionalAlbedo = maxComponent(sheenColor) *
      (0.157 + 0.343 * (1.0 - NdotV)) * (1.0 - sheenRoughness * 0.5);
    float baseLayerEnergy = (1.0 - clearcoatFactor * clearcoatViewFresnel) *
      (1.0 - clamp(sheenDirectionalAlbedo, 0.0, 1.0));
    diffuseColor *= baseLayerEnergy;
    specularColor *= baseLayerEnergy;

    // Compute reflectance.
    float reflectance = max(max(specularColor.r, specularColor.g), specularColor.b);

    // For typical incident reflectance range (between 4% to 100%) set the grazing
    // reflectance to 100% for typical fresnel effect.
    // For very low reflectance range on highly diffuse objects (below 4%),
    // incrementally reduce grazing reflecance to 0%.
    float reflectance90 = clamp(reflectance * 25.0, 0.0, 1.0);
    vec3 specularEnvironmentR0 = specularColor.rgb;
    vec3 specularEnvironmentR90 = vec3(1.0, 1.0, 1.0) * reflectance90;
    vec3 reflection = -normalize(reflect(v, n));

    PBRInfo pbrInfo = PBRInfo(
      0.0, // NdotL
      NdotV,
      0.0, // NdotH
      0.0, // LdotH
      0.0, // VdotH
      perceptualRoughness,
      metallic,
      specularEnvironmentR0,
      specularEnvironmentR90,
      alphaRoughness,
      diffuseColor,
      specularColor,
      n,
      v,
      n,
      n
    );


#ifdef USE_LIGHTS
    // Apply ambient light
    PBRInfo_setAmbientLight(pbrInfo);
    color += calculateMaterialLightColor(
      pbrInfo,
      lighting.ambientColor,
      clearcoatNormal,
      clearcoatFactor,
      clearcoatRoughness,
      sheenColor,
      sheenRoughness,
      anisotropyTangent,
      anisotropyStrength
    );

    // Apply directional light
    for(int i = 0; i < lighting.directionalLightCount; i++) {
      if (i < lighting.directionalLightCount) {
        PBRInfo_setDirectionalLight(pbrInfo, lighting_getDirectionalLight(i).direction);
        color += calculateMaterialLightColor(
          pbrInfo,
          lighting_getDirectionalLight(i).color,
          clearcoatNormal,
          clearcoatFactor,
          clearcoatRoughness,
          sheenColor,
          sheenRoughness,
          anisotropyTangent,
          anisotropyStrength
        );
        color += calculateDiffuseTransmissionLight(
          pbrInfo,
          lighting_getDirectionalLight(i).color,
          diffuseTransmissionColor,
          diffuseTransmission,
          multiscatterColor,
          thickness
        );
      }
    }

    // Apply point light
    for(int i = 0; i < lighting.pointLightCount; i++) {
      if (i < lighting.pointLightCount) {
        PBRInfo_setPointLight(pbrInfo, lighting_getPointLight(i));
        float attenuation = getPointLightAttenuation(lighting_getPointLight(i), distance(lighting_getPointLight(i).position, pbr_vPosition));
        color += calculateMaterialLightColor(
          pbrInfo,
          lighting_getPointLight(i).color / attenuation,
          clearcoatNormal,
          clearcoatFactor,
          clearcoatRoughness,
          sheenColor,
          sheenRoughness,
          anisotropyTangent,
          anisotropyStrength
        );
        color += calculateDiffuseTransmissionLight(
          pbrInfo,
          lighting_getPointLight(i).color / attenuation,
          diffuseTransmissionColor,
          diffuseTransmission,
          multiscatterColor,
          thickness
        );
      }
    }

    for(int i = 0; i < lighting.spotLightCount; i++) {
      if (i < lighting.spotLightCount) {
        PBRInfo_setSpotLight(pbrInfo, lighting_getSpotLight(i));
        float attenuation = getSpotLightAttenuation(lighting_getSpotLight(i), pbr_vPosition);
        color += calculateMaterialLightColor(
          pbrInfo,
          lighting_getSpotLight(i).color / attenuation,
          clearcoatNormal,
          clearcoatFactor,
          clearcoatRoughness,
          sheenColor,
          sheenRoughness,
          anisotropyTangent,
          anisotropyStrength
        );
        color += calculateDiffuseTransmissionLight(
          pbrInfo,
          lighting_getSpotLight(i).color / attenuation,
          diffuseTransmissionColor,
          diffuseTransmission,
          multiscatterColor,
          thickness
        );
      }
    }
#endif

    // Calculate lighting contribution from image based lighting source (IBL)
#ifdef USE_IBL
    if (pbrMaterial.IBLenabled) {
      color += getIBLContribution(
        pbrInfo,
        n,
        getAnisotropicReflection(pbrInfo, anisotropyTangent, anisotropyStrength)
      );
      color += calculateClearcoatIBLContribution(
        pbrInfo,
        clearcoatNormal,
        -normalize(reflect(v, clearcoatNormal)),
        clearcoatFactor,
        clearcoatRoughness
      );
      color += calculateDiffuseTransmissionIBL(
        pbrInfo,
        diffuseTransmissionColor,
        diffuseTransmission,
        multiscatterColor,
        thickness
      );
      color += sheenColor * pbrMaterial.scaleIBLAmbient.x * (1.0 - sheenRoughness) * 0.25;
    }
#endif

 // Apply optional PBR terms for additional (optional) shading
#ifdef HAS_OCCLUSIONMAP
    if (pbrMaterial.occlusionMapEnabled) {
      float ao = texture(pbr_occlusionSampler, occlusionUV).r;
      color = mix(color, color * ao, pbrMaterial.occlusionStrength);
    }
#endif

    vec3 emissive = pbrMaterial.emissiveFactor;
#ifdef HAS_EMISSIVEMAP
    if (pbrMaterial.emissiveMapEnabled) {
      emissive *= SRGBtoLINEAR(texture(pbr_emissiveSampler, emissiveUV)).rgb;
    }
#endif
    color += emissive * pbrMaterial.emissiveStrength;

    if (transmission > 0.0) {
#ifdef USE_TRANSMISSION_FRAMEBUFFER
      float dielectricFresnel = getDielectricF0(pbrMaterial.ior);
      float transmissionFresnel = dielectricFresnel +
        (1.0 - dielectricFresnel) * pow(1.0 - NdotV, 5.0);
      vec3 transmittedColor = getTransmittedSceneColor(
        pbr_vPosition,
        n,
        v,
        thickness,
        perceptualRoughness
      );
      color += transmittedColor * getVolumeAttenuation(thickness) *
        transmission * (1.0 - transmissionFresnel);
#else
      color = mix(color, color * getVolumeAttenuation(thickness), transmission);
#endif
    }

    // This section uses mix to override final color for reference app visualization
    // of various parameters in the lighting equation.
#ifdef PBR_DEBUG
    // TODO: Figure out how to debug multiple lights

    // color = mix(color, F, pbr_scaleFGDSpec.x);
    // color = mix(color, vec3(G), pbr_scaleFGDSpec.y);
    // color = mix(color, vec3(D), pbr_scaleFGDSpec.z);
    // color = mix(color, specContrib, pbr_scaleFGDSpec.w);

    // color = mix(color, diffuseContrib, pbr_scaleDiffBaseMR.x);
    color = mix(color, baseColor.rgb, pbrMaterial.scaleDiffBaseMR.y);
    color = mix(color, vec3(metallic), pbrMaterial.scaleDiffBaseMR.z);
    color = mix(color, vec3(perceptualRoughness), pbrMaterial.scaleDiffBaseMR.w);
#endif

  }

#ifdef USE_TRANSMISSION_FRAMEBUFFER
  float alpha = clamp(baseColor.a, 0.0, 1.0);
#else
  float alpha = clamp(baseColor.a * (1.0 - transmission), 0.0, 1.0);
#endif
  return vec4(applySceneColorManagement(color), alpha);
}
`,Xr=`struct PBRFragmentInputs {
  pbr_vPosition: vec3f,
  pbr_vUV0: vec2f,
  pbr_vUV1: vec2f,
  pbr_vTBN: mat3x3f,
  pbr_vNormal: vec3f
};

var<private> fragmentInputs: PBRFragmentInputs;

fn pbr_setPositionNormalTangentUV(
  position: vec4f,
  normal: vec4f,
  tangent: vec4f,
  uv0: vec2f,
  uv1: vec2f
)
{
  var pos: vec4f = pbrProjection.modelMatrix * position;
  fragmentInputs.pbr_vPosition = pos.xyz / pos.w;
  fragmentInputs.pbr_vNormal = vec3f(0.0, 0.0, 1.0);
  fragmentInputs.pbr_vTBN = mat3x3f(
    vec3f(1.0, 0.0, 0.0),
    vec3f(0.0, 1.0, 0.0),
    vec3f(0.0, 0.0, 1.0)
  );
  fragmentInputs.pbr_vUV0 = vec2f(0.0, 0.0);
  fragmentInputs.pbr_vUV1 = uv1;

#ifdef HAS_NORMALS
  let normalW: vec3f = normalize((pbrProjection.normalMatrix * vec4f(normal.xyz, 0.0)).xyz);
  fragmentInputs.pbr_vNormal = normalW;
#ifdef HAS_TANGENTS
  let tangentW: vec3f = normalize((pbrProjection.modelMatrix * vec4f(tangent.xyz, 0.0)).xyz);
  let bitangentW: vec3f = cross(normalW, tangentW) * tangent.w;
  fragmentInputs.pbr_vTBN = mat3x3f(tangentW, bitangentW, normalW);
#endif
#endif

#ifdef HAS_UV
  fragmentInputs.pbr_vUV0 = uv0;
#endif
}

struct pbrMaterialUniforms {
  // Material is unlit
  unlit: u32,

  // Base color map
  baseColorMapEnabled: u32,
  baseColorFactor: vec4f,

  normalMapEnabled : u32,
  normalScale: f32,  // #ifdef HAS_NORMALMAP

  emissiveMapEnabled: u32,
  emissiveFactor: vec3f, // #ifdef HAS_EMISSIVEMAP

  metallicRoughnessValues: vec2f,
  metallicRoughnessMapEnabled: u32,

  occlusionMapEnabled: i32,
  occlusionStrength: f32, // #ifdef HAS_OCCLUSIONMAP
  
  alphaCutoffEnabled: i32,
  alphaCutoff: f32, // #ifdef ALPHA_CUTOFF

  specularColorFactor: vec3f,
  specularIntensityFactor: f32,
  specularColorMapEnabled: i32,
  specularIntensityMapEnabled: i32,

  ior: f32,

  transmissionFactor: f32,
  transmissionMapEnabled: i32,

  thicknessFactor: f32,
  attenuationDistance: f32,
  attenuationColor: vec3f,

  clearcoatFactor: f32,
  clearcoatRoughnessFactor: f32,
  clearcoatMapEnabled: i32,
  clearcoatRoughnessMapEnabled: i32,

  sheenColorFactor: vec3f,
  sheenRoughnessFactor: f32,
  sheenColorMapEnabled: i32,
  sheenRoughnessMapEnabled: i32,

  iridescenceFactor: f32,
  iridescenceIor: f32,
  iridescenceThicknessRange: vec2f,
  iridescenceMapEnabled: i32,

  anisotropyStrength: f32,
  anisotropyRotation: f32,
  anisotropyDirection: vec2f,
  anisotropyMapEnabled: i32,

  emissiveStrength: f32,
  dispersion: f32,
  
  // IBL
  IBLenabled: i32,
  scaleIBLAmbient: vec2f, // #ifdef USE_IBL
  
  // debugging flags used for shader output of intermediate PBR variables
  // #ifdef PBR_DEBUG
  scaleDiffBaseMR: vec4f,
  scaleFGDSpec: vec4f,
  // #endif

  baseColorUVSet: i32,
  baseColorUVTransform: mat3x3f,
  metallicRoughnessUVSet: i32,
  metallicRoughnessUVTransform: mat3x3f,
  normalUVSet: i32,
  normalUVTransform: mat3x3f,
  occlusionUVSet: i32,
  occlusionUVTransform: mat3x3f,
  emissiveUVSet: i32,
  emissiveUVTransform: mat3x3f,
  specularColorUVSet: i32,
  specularColorUVTransform: mat3x3f,
  specularIntensityUVSet: i32,
  specularIntensityUVTransform: mat3x3f,
  transmissionUVSet: i32,
  transmissionUVTransform: mat3x3f,
  thicknessUVSet: i32,
  thicknessUVTransform: mat3x3f,
  clearcoatUVSet: i32,
  clearcoatUVTransform: mat3x3f,
  clearcoatRoughnessUVSet: i32,
  clearcoatRoughnessUVTransform: mat3x3f,
  clearcoatNormalUVSet: i32,
  clearcoatNormalUVTransform: mat3x3f,
  sheenColorUVSet: i32,
  sheenColorUVTransform: mat3x3f,
  sheenRoughnessUVSet: i32,
  sheenRoughnessUVTransform: mat3x3f,
  iridescenceUVSet: i32,
  iridescenceUVTransform: mat3x3f,
  iridescenceThicknessUVSet: i32,
  iridescenceThicknessUVTransform: mat3x3f,
  anisotropyUVSet: i32,
  anisotropyUVTransform: mat3x3f,

  bumpFactor: f32,
  bumpMapEnabled: i32,
  diffuseTransmissionFactor: f32,
  diffuseTransmissionMapEnabled: i32,
  diffuseTransmissionColorFactor: vec3f,
  diffuseTransmissionColorMapEnabled: i32,
  multiscatterColorFactor: vec3f,
  multiscatterColorMapEnabled: i32,
  scatterAnisotropy: f32,

  bumpUVSet: i32,
  bumpUVTransform: mat3x3f,
  diffuseTransmissionUVSet: i32,
  diffuseTransmissionUVTransform: mat3x3f,
  diffuseTransmissionColorUVSet: i32,
  diffuseTransmissionColorUVTransform: mat3x3f,
  multiscatterColorUVSet: i32,
  multiscatterColorUVTransform: mat3x3f,
}

@group(3) @binding(auto) var<uniform> pbrMaterial : pbrMaterialUniforms;

// Samplers
#ifdef HAS_BASECOLORMAP
@group(3) @binding(auto) var pbr_baseColorSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_baseColorSamplerSampler: sampler;
#endif
#ifdef HAS_NORMALMAP
@group(3) @binding(auto) var pbr_normalSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_normalSamplerSampler: sampler;
#endif
#ifdef HAS_EMISSIVEMAP
@group(3) @binding(auto) var pbr_emissiveSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_emissiveSamplerSampler: sampler;
#endif
#ifdef HAS_METALROUGHNESSMAP
@group(3) @binding(auto) var pbr_metallicRoughnessSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_metallicRoughnessSamplerSampler: sampler;
#endif
#ifdef HAS_OCCLUSIONMAP
@group(3) @binding(auto) var pbr_occlusionSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_occlusionSamplerSampler: sampler;
#endif
#ifdef HAS_SPECULARCOLORMAP
@group(3) @binding(auto) var pbr_specularColorSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_specularColorSamplerSampler: sampler;
#endif
#ifdef HAS_SPECULARINTENSITYMAP
@group(3) @binding(auto) var pbr_specularIntensitySampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_specularIntensitySamplerSampler: sampler;
#endif
#ifdef HAS_TRANSMISSIONMAP
@group(3) @binding(auto) var pbr_transmissionSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_transmissionSamplerSampler: sampler;
#endif
#ifdef HAS_THICKNESSMAP
@group(3) @binding(auto) var pbr_thicknessSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_thicknessSamplerSampler: sampler;
#endif
#ifdef HAS_CLEARCOATMAP
@group(3) @binding(auto) var pbr_clearcoatSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_clearcoatSamplerSampler: sampler;
#endif
#ifdef HAS_CLEARCOATROUGHNESSMAP
@group(3) @binding(auto) var pbr_clearcoatRoughnessSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_clearcoatRoughnessSamplerSampler: sampler;
#endif
#ifdef HAS_CLEARCOATNORMALMAP
@group(3) @binding(auto) var pbr_clearcoatNormalSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_clearcoatNormalSamplerSampler: sampler;
#endif
#ifdef HAS_SHEENCOLORMAP
@group(3) @binding(auto) var pbr_sheenColorSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_sheenColorSamplerSampler: sampler;
#endif
#ifdef HAS_SHEENROUGHNESSMAP
@group(3) @binding(auto) var pbr_sheenRoughnessSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_sheenRoughnessSamplerSampler: sampler;
#endif
#ifdef HAS_IRIDESCENCEMAP
@group(3) @binding(auto) var pbr_iridescenceSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_iridescenceSamplerSampler: sampler;
#endif
#ifdef HAS_IRIDESCENCETHICKNESSMAP
@group(3) @binding(auto) var pbr_iridescenceThicknessSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_iridescenceThicknessSamplerSampler: sampler;
#endif
#ifdef HAS_ANISOTROPYMAP
@group(3) @binding(auto) var pbr_anisotropySampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_anisotropySamplerSampler: sampler;
#endif
#ifdef HAS_BUMPMAP
@group(3) @binding(auto) var pbr_bumpSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_bumpSamplerSampler: sampler;
#endif
#ifdef HAS_DIFFUSETRANSMISSIONMAP
@group(3) @binding(auto) var pbr_diffuseTransmissionSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_diffuseTransmissionSamplerSampler: sampler;
#endif
#ifdef HAS_DIFFUSETRANSMISSIONCOLORMAP
@group(3) @binding(auto) var pbr_diffuseTransmissionColorSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_diffuseTransmissionColorSamplerSampler: sampler;
#endif
#ifdef HAS_MULTISCATTERCOLORMAP
@group(3) @binding(auto) var pbr_multiscatterColorSampler: texture_2d<f32>;
@group(3) @binding(auto) var pbr_multiscatterColorSamplerSampler: sampler;
#endif
// Encapsulate the various inputs used by the various functions in the shading equation
// We store values in this struct to simplify the integration of alternative implementations
// of the shading terms, outlined in the Readme.MD Appendix.
struct PBRInfo {
  NdotL: f32,                  // cos angle between normal and light direction
  NdotV: f32,                  // cos angle between normal and view direction
  NdotH: f32,                  // cos angle between normal and half vector
  LdotH: f32,                  // cos angle between light direction and half vector
  VdotH: f32,                  // cos angle between view direction and half vector
  perceptualRoughness: f32,    // roughness value, as authored by the model creator (input to shader)
  metalness: f32,              // metallic value at the surface
  reflectance0: vec3f,            // full reflectance color (normal incidence angle)
  reflectance90: vec3f,           // reflectance color at grazing angle
  alphaRoughness: f32,         // roughness mapped to a more linear change in the roughness (proposed by [2])
  diffuseColor: vec3f,            // color contribution from diffuse lighting
  specularColor: vec3f,           // color contribution from specular lighting
  n: vec3f,                       // normal at surface point
  v: vec3f,                       // vector from surface point to camera
  l: vec3f,                       // direction from the surface toward the current light
  h: vec3f                        // half vector between the current light and camera
};

const M_PI = 3.141592653589793;
const c_MinRoughness = 0.04;

// Widen sub-pixel specular lobes using the screen-space normal footprint.
// This is geometric specular antialiasing: the normal variance is converted
// into an additional squared perceptual roughness before evaluating BRDFs.
fn widenSpecularRoughness(perceptualRoughness: f32, normal: vec3f) -> f32 {
  let normalDerivativeX = dpdx(normal);
  let normalDerivativeY = dpdy(normal);
  let normalVariance =
    dot(normalDerivativeX, normalDerivativeX) +
    dot(normalDerivativeY, normalDerivativeY);
  let kernelRoughnessSquared = min(2.0 * normalVariance, 1.0);
  return clamp(
    sqrt(perceptualRoughness * perceptualRoughness + kernelRoughnessSquared),
    c_MinRoughness,
    1.0
  );
}

fn SRGBtoLINEAR(srgbIn: vec4f ) -> vec4f
{
  var linOut: vec3f = srgbIn.xyz;
#ifdef MANUAL_SRGB
  let bLess: vec3f = step(vec3f(0.04045), srgbIn.xyz);
  linOut = mix(
    srgbIn.xyz / vec3f(12.92),
    pow((srgbIn.xyz + vec3f(0.055)) / vec3f(1.055), vec3f(2.4)),
    bLess
  );
#ifdef SRGB_FAST_APPROXIMATION
  linOut = pow(srgbIn.xyz, vec3f(2.2));
#endif
#endif
  return vec4f(linOut, srgbIn.w);
}

fn getMaterialUV(uvSet: i32, uvTransform: mat3x3f) -> vec2f
{
  var baseUV = fragmentInputs.pbr_vUV0;
  if (uvSet == 1) {
    baseUV = fragmentInputs.pbr_vUV1;
  }
  return (uvTransform * vec3f(baseUV, 1.0)).xy;
}

// Build the tangent basis from interpolated attributes or screen-space derivatives.
fn getTBN(uv: vec2f) -> mat3x3f
{
  let pos_dx: vec3f = dpdx(fragmentInputs.pbr_vPosition);
  let pos_dy: vec3f = dpdy(fragmentInputs.pbr_vPosition);
  let tex_dx: vec3f = dpdx(vec3f(uv, 0.0));
  let tex_dy: vec3f = dpdy(vec3f(uv, 0.0));
  var t: vec3f = (tex_dy.y * pos_dx - tex_dx.y * pos_dy) / (tex_dx.x * tex_dy.y - tex_dy.x * tex_dx.y);

  var ng: vec3f = cross(pos_dy, pos_dx);
#ifdef HAS_NORMALS
  ng = normalize(fragmentInputs.pbr_vNormal);
#endif
  t = normalize(t - ng * dot(ng, t));
  var b: vec3f = normalize(cross(ng, t));
  var tbn: mat3x3f = mat3x3f(t, b, ng);
#ifdef HAS_TANGENTS
  tbn = fragmentInputs.pbr_vTBN;
#endif

  return tbn;
}

// Find the normal for this fragment, pulling either from a predefined normal map
// or from the interpolated mesh normal and tangent attributes.
fn getMappedNormal(
  normalSampler: texture_2d<f32>,
  normalSamplerBinding: sampler,
  tbn: mat3x3f,
  normalScale: f32,
  uv: vec2f
) -> vec3f
{
  let n = textureSample(normalSampler, normalSamplerBinding, uv).rgb;
  return normalize(tbn * ((2.0 * n - 1.0) * vec3f(normalScale, normalScale, 1.0)));
}

fn getNormal(tbn: mat3x3f, uv: vec2f) -> vec3f
{
  // The tbn matrix is linearly interpolated, so we need to re-normalize
  var n: vec3f = normalize(tbn[2].xyz);
#ifdef HAS_NORMALMAP
  n = getMappedNormal(
    pbr_normalSampler,
    pbr_normalSamplerSampler,
    tbn,
    pbrMaterial.normalScale,
    uv
  );
#endif

#ifdef HAS_BUMPMAP
  let bumpUV = getMaterialUV(pbrMaterial.bumpUVSet, pbrMaterial.bumpUVTransform);
  let bumpTexelSize = 1.0 / vec2f(textureDimensions(pbr_bumpSampler, 0));
  let bumpHeight = textureSample(pbr_bumpSampler, pbr_bumpSamplerSampler, bumpUV).r;
  let bumpGradient = vec2f(
    textureSample(
      pbr_bumpSampler,
      pbr_bumpSamplerSampler,
      bumpUV + vec2f(bumpTexelSize.x, 0.0)
    ).r - bumpHeight,
    textureSample(
      pbr_bumpSampler,
      pbr_bumpSamplerSampler,
      bumpUV + vec2f(0.0, bumpTexelSize.y)
    ).r - bumpHeight
  );
  n = normalize(n - pbrMaterial.bumpFactor *
    (tbn[0] * bumpGradient.x + tbn[1] * bumpGradient.y));
#endif

  return n;
}

fn getClearcoatNormal(tbn: mat3x3f, baseNormal: vec3f, uv: vec2f) -> vec3f
{
#ifdef HAS_CLEARCOATNORMALMAP
  return getMappedNormal(
    pbr_clearcoatNormalSampler,
    pbr_clearcoatNormalSamplerSampler,
    tbn,
    1.0,
    uv
  );
#else
  return baseNormal;
#endif
}

// Calculation of the lighting contribution from an optional Image Based Light source.
// Precomputed Environment Maps are required uniform inputs and are computed as outlined in [1].
// See our README.md on Environment Maps [3] for additional discussion.
#ifdef USE_IBL
fn getIBLContribution(pbrInfo: PBRInfo, n: vec3f, reflection: vec3f) -> vec3f
{
#ifdef USE_SCENE_ENVIRONMENT
  let maximumMipLevel = max(pbrScene.environmentMipCount - 1.0, 0.0);
  let rotationSine = sin(pbrScene.environmentRotation);
  let rotationCosine = cos(pbrScene.environmentRotation);
  let environmentRotation = mat2x2f(
    vec2f(rotationCosine, rotationSine),
    vec2f(-rotationSine, rotationCosine)
  );
  let rotatedNormal = environmentRotation * n.xz;
  let rotatedReflection = environmentRotation * reflection.xz;
  let environmentNormal = vec3f(rotatedNormal.x, n.y, rotatedNormal.y);
  let environmentReflection = vec3f(rotatedReflection.x, reflection.y, rotatedReflection.y);
#else
  let maximumMipLevel = 9.0;
  let environmentNormal = n;
  let environmentReflection = reflection;
#endif
  let lod = pbrInfo.perceptualRoughness * maximumMipLevel;
  // retrieve a scale and bias to F0. See [1], Figure 3
  let brdfSample = textureSampleLevel(
    pbr_brdfLUT,
    pbr_brdfLUTSampler,
    vec2f(pbrInfo.NdotV, 1.0 - pbrInfo.perceptualRoughness),
    0.0
  );
  let diffuseSample = textureSampleLevel(
    pbr_diffuseEnvSampler,
    pbr_diffuseEnvSamplerSampler,
    environmentNormal,
    0.0
  );
  var specularSample = textureSampleLevel(
    pbr_specularEnvSampler,
    pbr_specularEnvSamplerSampler,
    environmentReflection,
    0.0
  );
#ifdef USE_TEX_LOD
  specularSample = textureSampleLevel(
    pbr_specularEnvSampler,
    pbr_specularEnvSamplerSampler,
    environmentReflection,
    lod
  );
#endif

#ifdef USE_SCENE_ENVIRONMENT
  let brdf = brdfSample.rgb;
  let diffuseLight = diffuseSample.rgb;
  let specularLight = specularSample.rgb;
#else
  let brdf = SRGBtoLINEAR(brdfSample).rgb;
  let diffuseLight = SRGBtoLINEAR(diffuseSample).rgb;
  let specularLight = SRGBtoLINEAR(specularSample).rgb;
#endif

  let diffuse = diffuseLight * pbrInfo.diffuseColor * pbrMaterial.scaleIBLAmbient.x;
  let specular =
    specularLight * (pbrInfo.specularColor * brdf.x + brdf.y) * pbrMaterial.scaleIBLAmbient.y;

#ifdef USE_SCENE_ENVIRONMENT
  return (diffuse + specular) * max(pbrScene.environmentIntensity, 0.0);
#else
  return diffuse + specular;
#endif
}
#endif

// Basic Lambertian diffuse
// Implementation from Lambert's Photometria https://archive.org/details/lambertsphotome00lambgoog
// See also [1], Equation 1
fn diffuse(pbrInfo: PBRInfo) -> vec3<f32> {
  return pbrInfo.diffuseColor / M_PI;
}

// The following equation models the Fresnel reflectance term of the spec equation (aka F())
// Implementation of fresnel from [4], Equation 15
fn specularReflection(pbrInfo: PBRInfo) -> vec3<f32> {
  return pbrInfo.reflectance0 +
    (pbrInfo.reflectance90 - pbrInfo.reflectance0) *
    pow(clamp(1.0 - pbrInfo.VdotH, 0.0, 1.0), 5.0);
}

// This calculates the specular geometric attenuation (aka G()),
// where rougher material will reflect less light back to the viewer.
// This implementation is based on [1] Equation 4, and we adopt their modifications to
// alphaRoughness as input as originally proposed in [2].
fn geometricOcclusion(pbrInfo: PBRInfo) -> f32 {
  let NdotL: f32 = pbrInfo.NdotL;
  let NdotV: f32 = pbrInfo.NdotV;
  let r: f32 = pbrInfo.alphaRoughness;

  let attenuationL = 2.0 * NdotL / (NdotL + sqrt(r * r + (1.0 - r * r) * (NdotL * NdotL)));
  let attenuationV = 2.0 * NdotV / (NdotV + sqrt(r * r + (1.0 - r * r) * (NdotV * NdotV)));
  return attenuationL * attenuationV;
}

// The following equation(s) model the distribution of microfacet normals across
// the area being drawn (aka D())
// Implementation from "Average Irregularity Representation of a Roughened Surface
// for Ray Reflection" by T. S. Trowbridge, and K. P. Reitz
// Follows the distribution function recommended in the SIGGRAPH 2013 course notes
// from EPIC Games [1], Equation 3.
fn microfacetDistribution(pbrInfo: PBRInfo) -> f32 {
  let roughnessSq = pbrInfo.alphaRoughness * pbrInfo.alphaRoughness;
  let f = (pbrInfo.NdotH * roughnessSq - pbrInfo.NdotH) * pbrInfo.NdotH + 1.0;
  return roughnessSq / (M_PI * f * f);
}

fn maxComponent(value: vec3f) -> f32 {
  return max(max(value.r, value.g), value.b);
}

fn getDielectricF0(ior: f32) -> f32 {
  let clampedIor = max(ior, 1.0);
  let ratio = (clampedIor - 1.0) / (clampedIor + 1.0);
  return ratio * ratio;
}

fn normalizeDirection(direction: vec2f) -> vec2f {
  let directionLength = length(direction);
  if (directionLength > 0.0001) {
    return direction / directionLength;
  }

  return vec2f(1.0, 0.0);
}

fn rotateDirection(direction: vec2f, rotation: f32) -> vec2f {
  let s = sin(rotation);
  let c = cos(rotation);
  return vec2f(direction.x * c - direction.y * s, direction.x * s + direction.y * c);
}

fn encodeLinearSRGB(linearColor: vec3f) -> vec3f {
  let positiveColor = max(linearColor, vec3f(0.0));
  return select(
    positiveColor * 12.92,
    1.055 * pow(positiveColor, vec3f(1.0 / 2.4)) - 0.055,
    positiveColor > vec3f(0.0031308)
  );
}

fn toneMapKhronosPBRNeutral(inputColor: vec3f) -> vec3f {
  let startCompression = 0.76;
  let darkestChannel = min(inputColor.r, min(inputColor.g, inputColor.b));
  let offset = select(
    0.04,
    darkestChannel - 6.25 * darkestChannel * darkestChannel,
    darkestChannel < 0.08
  );
  var color = inputColor - vec3f(offset);
  let peak = maxComponent(color);
  if (peak < startCompression) {
    return color;
  }

  let compressionRange = 1.0 - startCompression;
  let compressedPeak = 1.0 - compressionRange * compressionRange /
    (peak + compressionRange - startCompression);
  color *= compressedPeak / max(peak, 0.0001);
  let desaturation = 1.0 - 1.0 / (0.15 * (peak - compressedPeak) + 1.0);
  return mix(color, vec3f(compressedPeak), desaturation);
}

fn applySceneColorManagement(sceneColor: vec3f) -> vec3f {
#ifdef USE_SCENE_COLOR_MANAGEMENT
  var color = max(sceneColor, vec3f(0.0)) * max(pbrScene.exposure, 0.0);
  if (pbrScene.toneMapMode == 1) {
    color /= vec3f(1.0) + color;
  } else if (pbrScene.toneMapMode == 2) {
    color = toneMapKhronosPBRNeutral(color);
  } else if (pbrScene.toneMapMode == 3) {
    color = clamp(
      (color * (2.51 * color + 0.03)) / (color * (2.43 * color + 0.59) + 0.14),
      vec3f(0.0),
      vec3f(1.0)
    );
  }
  if (pbrScene.outputEncoding == 0) {
    return color;
  }
  return encodeLinearSRGB(color);
#else
  return pow(max(sceneColor, vec3f(0.0)), vec3f(1.0 / 2.2));
#endif
}

fn dielectricSchlick(reflectance: f32, cosine: f32) -> f32 {
  return reflectance + (1.0 - reflectance) * pow(clamp(1.0 - cosine, 0.0, 1.0), 5.0);
}

fn evaluateIridescenceSensitivity(opticalPathDifference: f32, phaseShift: vec3f) -> vec3f {
  let phase = 2.0 * M_PI * opticalPathDifference * 1.0e-9;
  let sensitivity = vec3f(5.4856e-13, 4.4201e-13, 5.2481e-13);
  let position = vec3f(1.6810e6, 1.7953e6, 2.2084e6);
  let variance = vec3f(4.3278e9, 9.3046e9, 6.6121e9);
  var xyz = sensitivity * sqrt(2.0 * M_PI * variance) *
    cos(position * phase + phaseShift) * exp(-phase * phase * variance);
  xyz.x += 9.7470e-14 * sqrt(2.0 * M_PI * 4.5282e9) *
    cos(2.2399e6 * phase + phaseShift.x) * exp(-4.5282e9 * phase * phase);
  xyz /= 1.0685e-7;
  return mat3x3f(
    vec3f(3.2404542, -0.9692660, 0.0556434),
    vec3f(-1.5371385, 1.8760108, -0.2040259),
    vec3f(-0.4985314, 0.0415560, 1.0572252)
  ) * xyz;
}

fn getIridescenceTint(
  iridescence: f32,
  thickness: f32,
  NdotV: f32,
  baseReflectance: vec3f
) -> vec3f {
  if (iridescence <= 0.0 || thickness <= 0.0) {
    return baseReflectance;
  }

  let filmIor = max(pbrMaterial.iridescenceIor, 1.0);
  let sineSquared = (1.0 - NdotV * NdotV) / (filmIor * filmIor);
  let cosineSquared = 1.0 - sineSquared;
  if (cosineSquared <= 0.0) {
    return mix(baseReflectance, vec3f(1.0), iridescence);
  }
  let filmCosine = sqrt(cosineSquared);
  let firstInterfaceReflectance = dielectricSchlick(getDielectricF0(filmIor), NdotV);
  let transmittedEnergy = 1.0 - firstInterfaceReflectance;
  let squareRootReflectance = sqrt(clamp(baseReflectance, vec3f(0.0), vec3f(0.9999)));
  let baseIor = (vec3f(1.0) + squareRootReflectance) /
    (vec3f(1.0) - squareRootReflectance);
  var secondInterfaceF0 = (baseIor - vec3f(filmIor)) / (baseIor + vec3f(filmIor));
  secondInterfaceF0 *= secondInterfaceF0;
  let secondInterfaceReflectance = secondInterfaceF0 +
    (vec3f(1.0) - secondInterfaceF0) * pow(1.0 - filmCosine, 5.0);
  let phaseShift = vec3f(M_PI) + select(
    vec3f(0.0),
    vec3f(M_PI),
    baseIor < vec3f(filmIor)
  );
  let opticalPathDifference = 2.0 * filmIor * thickness * filmCosine;
  let combinedReflectance = clamp(
    firstInterfaceReflectance * secondInterfaceReflectance,
    vec3f(0.00001),
    vec3f(0.9999)
  );
  let recurringAmplitude = sqrt(combinedReflectance);
  let interfaceResponse = transmittedEnergy * transmittedEnergy * secondInterfaceReflectance /
    (vec3f(1.0) - combinedReflectance);
  var reflectedSpectrum = vec3f(firstInterfaceReflectance) + interfaceResponse;
  var harmonicAmplitude = interfaceResponse - vec3f(transmittedEnergy);
  for (var harmonic = 1; harmonic <= 2; harmonic++) {
    harmonicAmplitude *= recurringAmplitude;
    reflectedSpectrum += harmonicAmplitude * 2.0 * evaluateIridescenceSensitivity(
      f32(harmonic) * opticalPathDifference,
      f32(harmonic) * phaseShift
    );
  }
  return mix(baseReflectance, clamp(reflectedSpectrum, vec3f(0.0), vec3f(1.0)), iridescence);
}

fn getVolumeAttenuation(thickness: f32) -> vec3f {
  if (thickness <= 0.0) {
    return vec3f(1.0);
  }

  let attenuationCoefficient =
    -log(max(pbrMaterial.attenuationColor, vec3f(0.0001))) /
    max(pbrMaterial.attenuationDistance, 0.0001);
  return exp(-attenuationCoefficient * thickness);
}

// KHR_materials_volume_scatter is an active draft. This evaluates a local,
// thickness-aware single-scattering approximation rather than random walk.
fn getDiffuseTransmissionAttenuation(
  pbrInfo: PBRInfo,
  multiscatterColor: vec3f,
  thickness: f32
) -> vec3f {
  let volumeAttenuation = getVolumeAttenuation(thickness);
  let scatteringStrength = maxComponent(multiscatterColor);
  if (thickness <= 0.0 || scatteringStrength <= 0.0001) {
    return volumeAttenuation;
  }

  let anisotropy = clamp(pbrMaterial.scatterAnisotropy, -0.95, 0.95);
  let scatteringCosine = clamp(dot(-pbrInfo.v, pbrInfo.l), -1.0, 1.0);
  let phaseDenominator = max(
    1.0 + anisotropy * anisotropy - 2.0 * anisotropy * scatteringCosine,
    0.0001
  );
  let phaseWeight = clamp(
    (1.0 - anisotropy * anisotropy) / pow(phaseDenominator, 1.5),
    0.0,
    4.0
  );
  let scatteringDepth = thickness / max(pbrMaterial.attenuationDistance, 0.0001);
  let scatteringProbability = 1.0 - exp(-scatteringDepth);
  let scatteringColor = clamp(multiscatterColor, vec3f(0.0), vec3f(1.0));
  return mix(
    volumeAttenuation,
    volumeAttenuation * mix(vec3f(1.0), scatteringColor * phaseWeight, scatteringColor),
    scatteringProbability
  );
}

fn calculateDiffuseTransmissionLight(
  pbrInfo: PBRInfo,
  lightColor: vec3f,
  diffuseTransmissionColor: vec3f,
  diffuseTransmission: f32,
  multiscatterColor: vec3f,
  thickness: f32
) -> vec3f {
  let oppositeHemisphere = max(dot(-pbrInfo.n, pbrInfo.l), 0.0);
  if (oppositeHemisphere <= 0.0 || diffuseTransmission <= 0.0) {
    return vec3f(0.0);
  }

  let nonReflectedEnergy = vec3f(1.0) - clamp(pbrInfo.reflectance0, vec3f(0.0), vec3f(1.0));
  let attenuatedColor = getDiffuseTransmissionAttenuation(
    pbrInfo,
    multiscatterColor,
    thickness
  );
  return lightColor * diffuseTransmissionColor * nonReflectedEnergy *
    attenuatedColor * (diffuseTransmission * oppositeHemisphere / M_PI);
}

#ifdef USE_IBL
fn calculateDiffuseTransmissionIBL(
  pbrInfo: PBRInfo,
  diffuseTransmissionColor: vec3f,
  diffuseTransmission: f32,
  multiscatterColor: vec3f,
  thickness: f32
) -> vec3f {
  if (diffuseTransmission <= 0.0) {
    return vec3f(0.0);
  }

#ifdef USE_SCENE_ENVIRONMENT
  let rotationSine = sin(pbrScene.environmentRotation);
  let rotationCosine = cos(pbrScene.environmentRotation);
  let environmentRotation = mat2x2f(
    vec2f(rotationCosine, rotationSine),
    vec2f(-rotationSine, rotationCosine)
  );
  let rotatedNormal = environmentRotation * -pbrInfo.n.xz;
  let oppositeNormal = vec3f(rotatedNormal.x, -pbrInfo.n.y, rotatedNormal.y);
  let environmentColor = textureSampleLevel(
    pbr_diffuseEnvSampler,
    pbr_diffuseEnvSamplerSampler,
    oppositeNormal,
    0.0
  ).rgb * max(pbrScene.environmentIntensity, 0.0);
#else
  let environmentColor = SRGBtoLINEAR(
    textureSampleLevel(pbr_diffuseEnvSampler, pbr_diffuseEnvSamplerSampler, -pbrInfo.n, 0.0)
  ).rgb;
#endif
  let nonReflectedEnergy = vec3f(1.0) - clamp(pbrInfo.reflectance0, vec3f(0.0), vec3f(1.0));
  return environmentColor * diffuseTransmissionColor * nonReflectedEnergy *
    getDiffuseTransmissionAttenuation(pbrInfo, multiscatterColor, thickness) *
    diffuseTransmission * pbrMaterial.scaleIBLAmbient.x;
}
#endif

#ifdef USE_TRANSMISSION_FRAMEBUFFER
fn sampleTransmittedSceneColor(
  position: vec3f,
  normal: vec3f,
  viewDirection: vec3f,
  thickness: f32,
  perceptualRoughness: f32,
  indexOfRefraction: f32
) -> vec3f {
  let refractionDirection = refract(
    -viewDirection,
    normal,
    1.0 / max(indexOfRefraction, 1.0)
  );
  let refractedPosition = position + refractionDirection * thickness;
  let clipPosition = pbrScene.projectionMatrix *
    pbrScene.viewMatrix * vec4f(refractedPosition, 1.0);
  var textureCoordinate = clipPosition.xy / max(clipPosition.w, 0.0001) * 0.5 + 0.5;
  textureCoordinate.y = 1.0 - textureCoordinate.y;
  textureCoordinate = clamp(textureCoordinate, vec2f(0.001), vec2f(0.999));

  let blurRadius = perceptualRoughness * perceptualRoughness * 8.0 /
    max(pbrScene.framebufferSize, vec2f(1.0));
  var sceneColor = textureSampleLevel(
    pbr_transmissionFramebufferSampler,
    pbr_transmissionFramebufferSamplerSampler,
    textureCoordinate,
    0.0
  ).rgb * 0.4;
  sceneColor += textureSampleLevel(
    pbr_transmissionFramebufferSampler,
    pbr_transmissionFramebufferSamplerSampler,
    textureCoordinate + vec2f(blurRadius.x, 0.0),
    0.0
  ).rgb * 0.15;
  sceneColor += textureSampleLevel(
    pbr_transmissionFramebufferSampler,
    pbr_transmissionFramebufferSamplerSampler,
    textureCoordinate - vec2f(blurRadius.x, 0.0),
    0.0
  ).rgb * 0.15;
  sceneColor += textureSampleLevel(
    pbr_transmissionFramebufferSampler,
    pbr_transmissionFramebufferSamplerSampler,
    textureCoordinate + vec2f(0.0, blurRadius.y),
    0.0
  ).rgb * 0.15;
  sceneColor += textureSampleLevel(
    pbr_transmissionFramebufferSampler,
    pbr_transmissionFramebufferSamplerSampler,
    textureCoordinate - vec2f(0.0, blurRadius.y),
    0.0
  ).rgb * 0.15;
  return max(sceneColor, vec3f(0.0));
}

fn getTransmittedSceneColor(
  position: vec3f,
  normal: vec3f,
  viewDirection: vec3f,
  thickness: f32,
  perceptualRoughness: f32
) -> vec3f {
  if (pbrMaterial.dispersion <= 0.0) {
    return sampleTransmittedSceneColor(
      position,
      normal,
      viewDirection,
      thickness,
      perceptualRoughness,
      pbrMaterial.ior
    );
  }

  let halfSpread = (max(pbrMaterial.ior, 1.0) - 1.0) * 0.025 * pbrMaterial.dispersion;
  let indicesOfRefraction = max(
    vec3f(pbrMaterial.ior - halfSpread, pbrMaterial.ior, pbrMaterial.ior + halfSpread),
    vec3f(1.0)
  );
  return vec3f(
    sampleTransmittedSceneColor(
      position, normal, viewDirection, thickness, perceptualRoughness, indicesOfRefraction.r
    ).r,
    sampleTransmittedSceneColor(
      position, normal, viewDirection, thickness, perceptualRoughness, indicesOfRefraction.g
    ).g,
    sampleTransmittedSceneColor(
      position, normal, viewDirection, thickness, perceptualRoughness, indicesOfRefraction.b
    ).b
  );
}
#endif

fn createClearcoatPBRInfo(
  basePBRInfo: PBRInfo,
  clearcoatNormal: vec3f,
  clearcoatRoughness: f32
) -> PBRInfo {
  let perceptualRoughness = clamp(clearcoatRoughness, c_MinRoughness, 1.0);
  let alphaRoughness = perceptualRoughness * perceptualRoughness;
  let NdotV = clamp(abs(dot(clearcoatNormal, basePBRInfo.v)), 0.001, 1.0);

  return PBRInfo(
    basePBRInfo.NdotL,
    NdotV,
    basePBRInfo.NdotH,
    basePBRInfo.LdotH,
    basePBRInfo.VdotH,
    perceptualRoughness,
    0.0,
    vec3f(0.04),
    vec3f(1.0),
    alphaRoughness,
    vec3f(0.0),
    vec3f(0.04),
    clearcoatNormal,
    basePBRInfo.v,
    basePBRInfo.l,
    basePBRInfo.h
  );
}

fn calculateClearcoatContribution(
  pbrInfo: PBRInfo,
  lightColor: vec3f,
  clearcoatNormal: vec3f,
  clearcoatFactor: f32,
  clearcoatRoughness: f32
) -> vec3f {
  if (clearcoatFactor <= 0.0) {
    return vec3f(0.0);
  }

  let clearcoatPBRInfo = createClearcoatPBRInfo(pbrInfo, clearcoatNormal, clearcoatRoughness);
  return calculateFinalColor(clearcoatPBRInfo, lightColor) * clearcoatFactor;
}

#ifdef USE_IBL
fn calculateClearcoatIBLContribution(
  pbrInfo: PBRInfo,
  clearcoatNormal: vec3f,
  reflection: vec3f,
  clearcoatFactor: f32,
  clearcoatRoughness: f32
) -> vec3f {
  if (clearcoatFactor <= 0.0) {
    return vec3f(0.0);
  }

  let clearcoatPBRInfo = createClearcoatPBRInfo(pbrInfo, clearcoatNormal, clearcoatRoughness);
  return getIBLContribution(clearcoatPBRInfo, clearcoatNormal, reflection) * clearcoatFactor;
}
#endif

fn calculateSheenContribution(
  pbrInfo: PBRInfo,
  lightColor: vec3f,
  sheenColor: vec3f,
  sheenRoughness: f32
) -> vec3f {
  if (maxComponent(sheenColor) <= 0.0) {
    return vec3f(0.0);
  }

  let alpha = max(sheenRoughness * sheenRoughness, 0.0001);
  let inverseAlpha = 1.0 / alpha;
  let sineSquared = max(1.0 - pbrInfo.NdotH * pbrInfo.NdotH, 0.0);
  let distribution = (2.0 + inverseAlpha) * pow(sineSquared, inverseAlpha * 0.5) /
    (2.0 * M_PI);
  let visibility = 1.0 / max(
    4.0 * (pbrInfo.NdotL + pbrInfo.NdotV - pbrInfo.NdotL * pbrInfo.NdotV),
    0.0001
  );
  return pbrInfo.NdotL * lightColor * sheenColor * distribution * visibility *
    (1.0 - pbrInfo.metalness);
}

fn calculateAnisotropicLightColor(
  pbrInfo: PBRInfo,
  lightColor: vec3f,
  anisotropyTangent: vec3f,
  anisotropyStrength: f32
) -> vec3f {
  if (anisotropyStrength <= 0.0) {
    return calculateFinalColor(pbrInfo, lightColor);
  }

  let anisotropyBitangent = normalize(cross(pbrInfo.n, anisotropyTangent));
  let tangentRoughness = mix(
    pbrInfo.alphaRoughness,
    1.0,
    anisotropyStrength * anisotropyStrength
  );
  let bitangentRoughness = clamp(pbrInfo.alphaRoughness, 0.001, 1.0);
  let roughnessProduct = tangentRoughness * bitangentRoughness;
  let distributionVector = vec3f(
    bitangentRoughness * dot(anisotropyTangent, pbrInfo.h),
    tangentRoughness * dot(anisotropyBitangent, pbrInfo.h),
    roughnessProduct * pbrInfo.NdotH
  );
  let distributionFactor = roughnessProduct /
    max(dot(distributionVector, distributionVector), 0.000001);
  let distribution = roughnessProduct * distributionFactor * distributionFactor / M_PI;
  let viewMask = pbrInfo.NdotL * length(vec3f(
    tangentRoughness * dot(anisotropyTangent, pbrInfo.v),
    bitangentRoughness * dot(anisotropyBitangent, pbrInfo.v),
    pbrInfo.NdotV
  ));
  let lightMask = pbrInfo.NdotV * length(vec3f(
    tangentRoughness * dot(anisotropyTangent, pbrInfo.l),
    bitangentRoughness * dot(anisotropyBitangent, pbrInfo.l),
    pbrInfo.NdotL
  ));
  let visibility = clamp(0.5 / max(viewMask + lightMask, 0.000001), 0.0, 1.0);
  let fresnel = specularReflection(pbrInfo);
  let diffuseContribution = (vec3f(1.0) - fresnel) * diffuse(pbrInfo);
  return pbrInfo.NdotL * lightColor *
    (diffuseContribution + fresnel * distribution * visibility);
}

fn getAnisotropicReflection(
  pbrInfo: PBRInfo,
  anisotropyTangent: vec3f,
  anisotropyStrength: f32
) -> vec3f {
  if (anisotropyStrength <= 0.0) {
    return -normalize(reflect(pbrInfo.v, pbrInfo.n));
  }
  let anisotropyBitangent = normalize(cross(pbrInfo.n, anisotropyTangent));
  var anisotropicNormal = normalize(cross(anisotropyBitangent, pbrInfo.v));
  anisotropicNormal = normalize(cross(anisotropicNormal, anisotropyBitangent));
  let bend = anisotropyStrength * (1.0 - pbrInfo.perceptualRoughness);
  return -normalize(reflect(pbrInfo.v, normalize(mix(pbrInfo.n, anisotropicNormal, bend))));
}

fn calculateMaterialLightColor(
  pbrInfo: PBRInfo,
  lightColor: vec3f,
  clearcoatNormal: vec3f,
  clearcoatFactor: f32,
  clearcoatRoughness: f32,
  sheenColor: vec3f,
  sheenRoughness: f32,
  anisotropyTangent: vec3f,
  anisotropyStrength: f32
) -> vec3f {
  var color = calculateAnisotropicLightColor(
    pbrInfo,
    lightColor,
    anisotropyTangent,
    anisotropyStrength
  );
  color += calculateClearcoatContribution(
    pbrInfo,
    lightColor,
    clearcoatNormal,
    clearcoatFactor,
    clearcoatRoughness
  );
  color += calculateSheenContribution(pbrInfo, lightColor, sheenColor, sheenRoughness);
  return color;
}

fn PBRInfo_setAmbientLight(pbrInfo: ptr<function, PBRInfo>) {
  (*pbrInfo).NdotL = 1.0;
  (*pbrInfo).NdotH = 0.0;
  (*pbrInfo).LdotH = 0.0;
  (*pbrInfo).VdotH = 1.0;
  (*pbrInfo).l = (*pbrInfo).n;
  (*pbrInfo).h = (*pbrInfo).n;
}

fn PBRInfo_setDirectionalLight(pbrInfo: ptr<function, PBRInfo>, lightDirection: vec3<f32>) {
  let n = (*pbrInfo).n;
  let v = (*pbrInfo).v;
  let l = normalize(lightDirection);             // Vector from surface point to light
  let h = normalize(l + v);                      // Half vector between both l and v

  (*pbrInfo).NdotL = clamp(dot(n, l), 0.001, 1.0);
  (*pbrInfo).NdotH = clamp(dot(n, h), 0.0, 1.0);
  (*pbrInfo).LdotH = clamp(dot(l, h), 0.0, 1.0);
  (*pbrInfo).VdotH = clamp(dot(v, h), 0.0, 1.0);
  (*pbrInfo).l = l;
  (*pbrInfo).h = h;
}

fn PBRInfo_setPointLight(pbrInfo: ptr<function, PBRInfo>, pointLight: PointLight) {
  let light_direction = normalize(pointLight.position - fragmentInputs.pbr_vPosition);
  PBRInfo_setDirectionalLight(pbrInfo, light_direction);
}

fn PBRInfo_setSpotLight(pbrInfo: ptr<function, PBRInfo>, spotLight: SpotLight) {
  let light_direction = normalize(spotLight.position - fragmentInputs.pbr_vPosition);
  PBRInfo_setDirectionalLight(pbrInfo, light_direction);
}

fn calculateFinalColor(pbrInfo: PBRInfo, lightColor: vec3<f32>) -> vec3<f32> {
  // Calculate the shading terms for the microfacet specular shading model
  let F = specularReflection(pbrInfo);
  let G = geometricOcclusion(pbrInfo);
  let D = microfacetDistribution(pbrInfo);

  // Calculation of analytical lighting contribution
  let diffuseContrib = (1.0 - F) * diffuse(pbrInfo);
  let specContrib = F * G * D / (4.0 * pbrInfo.NdotL * pbrInfo.NdotV);
  // Obtain final intensity as reflectance (BRDF) scaled by the energy of the light (cosine law)
  return pbrInfo.NdotL * lightColor * (diffuseContrib + specContrib);
}

fn pbr_filterColor(vertexColor: vec4<f32>) -> vec4<f32> {
  let baseColorUV = getMaterialUV(pbrMaterial.baseColorUVSet, pbrMaterial.baseColorUVTransform);
  let metallicRoughnessUV = getMaterialUV(
    pbrMaterial.metallicRoughnessUVSet,
    pbrMaterial.metallicRoughnessUVTransform
  );
  let normalUV = getMaterialUV(pbrMaterial.normalUVSet, pbrMaterial.normalUVTransform);
  let occlusionUV = getMaterialUV(pbrMaterial.occlusionUVSet, pbrMaterial.occlusionUVTransform);
  let emissiveUV = getMaterialUV(pbrMaterial.emissiveUVSet, pbrMaterial.emissiveUVTransform);
  let specularColorUV = getMaterialUV(
    pbrMaterial.specularColorUVSet,
    pbrMaterial.specularColorUVTransform
  );
  let specularIntensityUV = getMaterialUV(
    pbrMaterial.specularIntensityUVSet,
    pbrMaterial.specularIntensityUVTransform
  );
  let transmissionUV = getMaterialUV(
    pbrMaterial.transmissionUVSet,
    pbrMaterial.transmissionUVTransform
  );
  let thicknessUV = getMaterialUV(pbrMaterial.thicknessUVSet, pbrMaterial.thicknessUVTransform);
  let clearcoatUV = getMaterialUV(pbrMaterial.clearcoatUVSet, pbrMaterial.clearcoatUVTransform);
  let clearcoatRoughnessUV = getMaterialUV(
    pbrMaterial.clearcoatRoughnessUVSet,
    pbrMaterial.clearcoatRoughnessUVTransform
  );
  let clearcoatNormalUV = getMaterialUV(
    pbrMaterial.clearcoatNormalUVSet,
    pbrMaterial.clearcoatNormalUVTransform
  );
  let sheenColorUV = getMaterialUV(
    pbrMaterial.sheenColorUVSet,
    pbrMaterial.sheenColorUVTransform
  );
  let sheenRoughnessUV = getMaterialUV(
    pbrMaterial.sheenRoughnessUVSet,
    pbrMaterial.sheenRoughnessUVTransform
  );
  let iridescenceUV = getMaterialUV(
    pbrMaterial.iridescenceUVSet,
    pbrMaterial.iridescenceUVTransform
  );
  let iridescenceThicknessUV = getMaterialUV(
    pbrMaterial.iridescenceThicknessUVSet,
    pbrMaterial.iridescenceThicknessUVTransform
  );
  let anisotropyUV = getMaterialUV(
    pbrMaterial.anisotropyUVSet,
    pbrMaterial.anisotropyUVTransform
  );
  let diffuseTransmissionUV = getMaterialUV(
    pbrMaterial.diffuseTransmissionUVSet,
    pbrMaterial.diffuseTransmissionUVTransform
  );
  let diffuseTransmissionColorUV = getMaterialUV(
    pbrMaterial.diffuseTransmissionColorUVSet,
    pbrMaterial.diffuseTransmissionColorUVTransform
  );
  let multiscatterColorUV = getMaterialUV(
    pbrMaterial.multiscatterColorUVSet,
    pbrMaterial.multiscatterColorUVTransform
  );

  // The albedo may be defined from a base texture or a flat color
  var baseColor: vec4<f32> = pbrMaterial.baseColorFactor * vertexColor;
  #ifdef HAS_BASECOLORMAP
  baseColor = SRGBtoLINEAR(
    textureSample(pbr_baseColorSampler, pbr_baseColorSamplerSampler, baseColorUV)
  ) * pbrMaterial.baseColorFactor * vertexColor;
  #endif

  #ifdef ALPHA_CUTOFF
  if (baseColor.a < pbrMaterial.alphaCutoff) {
    discard;
  }
  #endif

  var color = vec3<f32>(0.0, 0.0, 0.0);
  var transmission = 0.0;

  if (pbrMaterial.unlit != 0u) {
    color = baseColor.rgb;
  } else {
    // Metallic and Roughness material properties are packed together
    // In glTF, these factors can be specified by fixed scalar values
    // or from a metallic-roughness map
    var perceptualRoughness = pbrMaterial.metallicRoughnessValues.y;
    var metallic = pbrMaterial.metallicRoughnessValues.x;
    #ifdef HAS_METALROUGHNESSMAP
    // Roughness is stored in the 'g' channel, metallic is stored in the 'b' channel.
    // This layout intentionally reserves the 'r' channel for (optional) occlusion map data
    let mrSample = textureSample(
      pbr_metallicRoughnessSampler,
      pbr_metallicRoughnessSamplerSampler,
      metallicRoughnessUV
    );
    perceptualRoughness = mrSample.g * perceptualRoughness;
    metallic = mrSample.b * metallic;
    #endif
    perceptualRoughness = clamp(perceptualRoughness, c_MinRoughness, 1.0);
    metallic = clamp(metallic, 0.0, 1.0);
    let tbn = getTBN(normalUV);
    let n = getNormal(tbn, normalUV);                          // normal at surface point
    perceptualRoughness = widenSpecularRoughness(perceptualRoughness, n);
    let v = normalize(pbrProjection.camera - fragmentInputs.pbr_vPosition);  // Vector from surface point to camera
    let NdotV = clamp(abs(dot(n, v)), 0.001, 1.0);
    var useExtendedPBR = false;
    #ifdef USE_MATERIAL_EXTENSIONS
    useExtendedPBR =
      pbrMaterial.specularColorMapEnabled != 0 ||
      pbrMaterial.specularIntensityMapEnabled != 0 ||
      abs(pbrMaterial.specularIntensityFactor - 1.0) > 0.0001 ||
      maxComponent(abs(pbrMaterial.specularColorFactor - vec3f(1.0))) > 0.0001 ||
      abs(pbrMaterial.ior - 1.5) > 0.0001 ||
      pbrMaterial.dispersion > 0.0001 ||
      pbrMaterial.transmissionMapEnabled != 0 ||
      pbrMaterial.transmissionFactor > 0.0001 ||
      pbrMaterial.diffuseTransmissionMapEnabled != 0 ||
      pbrMaterial.diffuseTransmissionColorMapEnabled != 0 ||
      pbrMaterial.diffuseTransmissionFactor > 0.0001 ||
      pbrMaterial.multiscatterColorMapEnabled != 0 ||
      maxComponent(pbrMaterial.multiscatterColorFactor) > 0.0001 ||
      pbrMaterial.clearcoatMapEnabled != 0 ||
      pbrMaterial.clearcoatRoughnessMapEnabled != 0 ||
      pbrMaterial.clearcoatFactor > 0.0001 ||
      pbrMaterial.clearcoatRoughnessFactor > 0.0001 ||
      pbrMaterial.sheenColorMapEnabled != 0 ||
      pbrMaterial.sheenRoughnessMapEnabled != 0 ||
      maxComponent(pbrMaterial.sheenColorFactor) > 0.0001 ||
      pbrMaterial.sheenRoughnessFactor > 0.0001 ||
      pbrMaterial.iridescenceMapEnabled != 0 ||
      pbrMaterial.iridescenceFactor > 0.0001 ||
      abs(pbrMaterial.iridescenceIor - 1.3) > 0.0001 ||
      abs(pbrMaterial.iridescenceThicknessRange.x - 100.0) > 0.0001 ||
      abs(pbrMaterial.iridescenceThicknessRange.y - 400.0) > 0.0001 ||
      pbrMaterial.anisotropyMapEnabled != 0 ||
      pbrMaterial.anisotropyStrength > 0.0001 ||
      abs(pbrMaterial.anisotropyRotation) > 0.0001 ||
      length(pbrMaterial.anisotropyDirection - vec2f(1.0, 0.0)) > 0.0001;
    #endif

    if (!useExtendedPBR) {
      let alphaRoughness = perceptualRoughness * perceptualRoughness;

      let f0 = vec3<f32>(0.04);
      var diffuseColor = baseColor.rgb * (vec3<f32>(1.0) - f0);
      diffuseColor *= 1.0 - metallic;
      let specularColor = mix(f0, baseColor.rgb, metallic);

      let reflectance = max(max(specularColor.r, specularColor.g), specularColor.b);
      let reflectance90 = clamp(reflectance * 25.0, 0.0, 1.0);
      let specularEnvironmentR0 = specularColor;
      let specularEnvironmentR90 = vec3<f32>(1.0, 1.0, 1.0) * reflectance90;
      let reflection = -normalize(reflect(v, n));

      var pbrInfo = PBRInfo(
        0.0, // NdotL
        NdotV,
        0.0, // NdotH
        0.0, // LdotH
        0.0, // VdotH
        perceptualRoughness,
        metallic,
        specularEnvironmentR0,
        specularEnvironmentR90,
        alphaRoughness,
        diffuseColor,
        specularColor,
        n,
        v,
        n,
        n
      );

      #ifdef USE_LIGHTS
      PBRInfo_setAmbientLight(&pbrInfo);
      color += calculateFinalColor(pbrInfo, lighting.ambientColor);

      for (var i = 0; i < lighting.directionalLightCount; i++) {
        if (i < lighting.directionalLightCount) {
          PBRInfo_setDirectionalLight(&pbrInfo, lighting_getDirectionalLight(i).direction);
          color += calculateFinalColor(pbrInfo, lighting_getDirectionalLight(i).color);
        }
      }

      for (var i = 0; i < lighting.pointLightCount; i++) {
        if (i < lighting.pointLightCount) {
          PBRInfo_setPointLight(&pbrInfo, lighting_getPointLight(i));
          let attenuation = getPointLightAttenuation(
            lighting_getPointLight(i),
            distance(lighting_getPointLight(i).position, fragmentInputs.pbr_vPosition)
          );
          color += calculateFinalColor(pbrInfo, lighting_getPointLight(i).color / attenuation);
        }
      }

      for (var i = 0; i < lighting.spotLightCount; i++) {
        if (i < lighting.spotLightCount) {
          PBRInfo_setSpotLight(&pbrInfo, lighting_getSpotLight(i));
          let attenuation = getSpotLightAttenuation(
            lighting_getSpotLight(i),
            fragmentInputs.pbr_vPosition
          );
          color += calculateFinalColor(pbrInfo, lighting_getSpotLight(i).color / attenuation);
        }
      }
      #endif

      #ifdef USE_IBL
      if (pbrMaterial.IBLenabled != 0) {
        color += getIBLContribution(pbrInfo, n, reflection);
      }
      #endif

      #ifdef HAS_OCCLUSIONMAP
      if (pbrMaterial.occlusionMapEnabled != 0) {
        let ao = textureSample(pbr_occlusionSampler, pbr_occlusionSamplerSampler, occlusionUV).r;
        color = mix(color, color * ao, pbrMaterial.occlusionStrength);
      }
      #endif

      var emissive = pbrMaterial.emissiveFactor;
      #ifdef HAS_EMISSIVEMAP
      if (pbrMaterial.emissiveMapEnabled != 0u) {
        emissive *= SRGBtoLINEAR(
          textureSample(pbr_emissiveSampler, pbr_emissiveSamplerSampler, emissiveUV)
        ).rgb;
      }
      #endif
      color += emissive * pbrMaterial.emissiveStrength;

      #ifdef PBR_DEBUG
      color = mix(color, baseColor.rgb, pbrMaterial.scaleDiffBaseMR.y);
      color = mix(color, vec3<f32>(metallic), pbrMaterial.scaleDiffBaseMR.z);
      color = mix(color, vec3<f32>(perceptualRoughness), pbrMaterial.scaleDiffBaseMR.w);
      #endif

      return vec4<f32>(applySceneColorManagement(color), baseColor.a);
    }

    var specularIntensity = pbrMaterial.specularIntensityFactor;
    #ifdef HAS_SPECULARINTENSITYMAP
    if (pbrMaterial.specularIntensityMapEnabled != 0) {
      specularIntensity *= textureSample(
        pbr_specularIntensitySampler,
        pbr_specularIntensitySamplerSampler,
        specularIntensityUV
      ).a;
    }
    #endif

    var specularFactor = pbrMaterial.specularColorFactor;
    #ifdef HAS_SPECULARCOLORMAP
    if (pbrMaterial.specularColorMapEnabled != 0) {
      specularFactor *= SRGBtoLINEAR(
        textureSample(
          pbr_specularColorSampler,
          pbr_specularColorSamplerSampler,
          specularColorUV
        )
      ).rgb;
    }
    #endif

    transmission = pbrMaterial.transmissionFactor;
    #ifdef HAS_TRANSMISSIONMAP
    if (pbrMaterial.transmissionMapEnabled != 0) {
      transmission *= textureSample(
        pbr_transmissionSampler,
        pbr_transmissionSamplerSampler,
        transmissionUV
      ).r;
    }
    #endif
    transmission = clamp(transmission * (1.0 - metallic), 0.0, 1.0);
    var thickness = max(pbrMaterial.thicknessFactor, 0.0);
    #ifdef HAS_THICKNESSMAP
    thickness *= textureSample(
      pbr_thicknessSampler,
      pbr_thicknessSamplerSampler,
      thicknessUV
    ).g;
    #endif

    var diffuseTransmission = clamp(pbrMaterial.diffuseTransmissionFactor, 0.0, 1.0);
    #ifdef HAS_DIFFUSETRANSMISSIONMAP
    if (pbrMaterial.diffuseTransmissionMapEnabled != 0) {
      diffuseTransmission *= textureSample(
        pbr_diffuseTransmissionSampler,
        pbr_diffuseTransmissionSamplerSampler,
        diffuseTransmissionUV
      ).a;
    }
    #endif
    diffuseTransmission *= (1.0 - metallic) * (1.0 - transmission);
    var diffuseTransmissionColor = pbrMaterial.diffuseTransmissionColorFactor;
    #ifdef HAS_DIFFUSETRANSMISSIONCOLORMAP
    if (pbrMaterial.diffuseTransmissionColorMapEnabled != 0) {
      diffuseTransmissionColor *= SRGBtoLINEAR(
        textureSample(
          pbr_diffuseTransmissionColorSampler,
          pbr_diffuseTransmissionColorSamplerSampler,
          diffuseTransmissionColorUV
        )
      ).rgb;
    }
    #endif
    var multiscatterColor = pbrMaterial.multiscatterColorFactor;
    #ifdef HAS_MULTISCATTERCOLORMAP
    if (pbrMaterial.multiscatterColorMapEnabled != 0) {
      multiscatterColor *= SRGBtoLINEAR(
        textureSample(
          pbr_multiscatterColorSampler,
          pbr_multiscatterColorSamplerSampler,
          multiscatterColorUV
        )
      ).rgb;
    }
    #endif

    var clearcoatFactor = pbrMaterial.clearcoatFactor;
    var clearcoatRoughness = pbrMaterial.clearcoatRoughnessFactor;
    #ifdef HAS_CLEARCOATMAP
    if (pbrMaterial.clearcoatMapEnabled != 0) {
      clearcoatFactor *= textureSample(
        pbr_clearcoatSampler,
        pbr_clearcoatSamplerSampler,
        clearcoatUV
      ).r;
    }
    #endif
    #ifdef HAS_CLEARCOATROUGHNESSMAP
    if (pbrMaterial.clearcoatRoughnessMapEnabled != 0) {
      clearcoatRoughness *= textureSample(
        pbr_clearcoatRoughnessSampler,
        pbr_clearcoatRoughnessSamplerSampler,
        clearcoatRoughnessUV
      ).g;
    }
    #endif
    clearcoatFactor = clamp(clearcoatFactor, 0.0, 1.0);
    clearcoatRoughness = clamp(clearcoatRoughness, c_MinRoughness, 1.0);
    let clearcoatNormal = getClearcoatNormal(getTBN(clearcoatNormalUV), n, clearcoatNormalUV);
    clearcoatRoughness = widenSpecularRoughness(clearcoatRoughness, clearcoatNormal);

    var sheenColor = pbrMaterial.sheenColorFactor;
    var sheenRoughness = pbrMaterial.sheenRoughnessFactor;
    #ifdef HAS_SHEENCOLORMAP
    if (pbrMaterial.sheenColorMapEnabled != 0) {
      sheenColor *= SRGBtoLINEAR(
        textureSample(
          pbr_sheenColorSampler,
          pbr_sheenColorSamplerSampler,
          sheenColorUV
        )
      ).rgb;
    }
    #endif
    #ifdef HAS_SHEENROUGHNESSMAP
    if (pbrMaterial.sheenRoughnessMapEnabled != 0) {
      sheenRoughness *= textureSample(
        pbr_sheenRoughnessSampler,
        pbr_sheenRoughnessSamplerSampler,
        sheenRoughnessUV
      ).a;
    }
    #endif
    sheenRoughness = clamp(sheenRoughness, c_MinRoughness, 1.0);

    var iridescence = pbrMaterial.iridescenceFactor;
    #ifdef HAS_IRIDESCENCEMAP
    if (pbrMaterial.iridescenceMapEnabled != 0) {
      iridescence *= textureSample(
        pbr_iridescenceSampler,
        pbr_iridescenceSamplerSampler,
        iridescenceUV
      ).r;
    }
    #endif
    iridescence = clamp(iridescence, 0.0, 1.0);
    var iridescenceThickness = mix(
      pbrMaterial.iridescenceThicknessRange.x,
      pbrMaterial.iridescenceThicknessRange.y,
      0.5
    );
    #ifdef HAS_IRIDESCENCETHICKNESSMAP
    iridescenceThickness = mix(
      pbrMaterial.iridescenceThicknessRange.x,
      pbrMaterial.iridescenceThicknessRange.y,
      textureSample(
        pbr_iridescenceThicknessSampler,
        pbr_iridescenceThicknessSamplerSampler,
        iridescenceThicknessUV
      ).g
    );
    #endif

    var anisotropyStrength = clamp(pbrMaterial.anisotropyStrength, 0.0, 1.0);
    var anisotropyDirection = normalizeDirection(pbrMaterial.anisotropyDirection);
    #ifdef HAS_ANISOTROPYMAP
    if (pbrMaterial.anisotropyMapEnabled != 0) {
      let anisotropySample = textureSample(
        pbr_anisotropySampler,
        pbr_anisotropySamplerSampler,
        anisotropyUV
      ).rgb;
      anisotropyStrength *= anisotropySample.b;
      let mappedDirection = anisotropySample.rg * 2.0 - 1.0;
      if (length(mappedDirection) > 0.0001) {
        anisotropyDirection = normalize(mappedDirection);
      }
    }
    #endif
    anisotropyDirection = rotateDirection(anisotropyDirection, pbrMaterial.anisotropyRotation);
    var anisotropyTangent =
      normalize(tbn[0] * anisotropyDirection.x + tbn[1] * anisotropyDirection.y);
    if (length(anisotropyTangent) < 0.0001) {
      anisotropyTangent = normalize(tbn[0]);
    }
    // Roughness is authored as perceptual roughness; as is convention,
    // convert to material roughness by squaring the perceptual roughness [2].
    let alphaRoughness = perceptualRoughness * perceptualRoughness;

    let dielectricF0 = getDielectricF0(pbrMaterial.ior);
    var dielectricSpecularF0 = min(
      vec3f(dielectricF0) * specularFactor * specularIntensity,
      vec3f(1.0)
    );
    dielectricSpecularF0 = getIridescenceTint(
      iridescence,
      iridescenceThickness,
      NdotV,
      dielectricSpecularF0
    );
    var diffuseColor = baseColor.rgb * (vec3f(1.0) - dielectricSpecularF0);
    diffuseColor *= (1.0 - metallic) * (1.0 - transmission) * (1.0 - diffuseTransmission);
    var specularColor = mix(dielectricSpecularF0, baseColor.rgb, metallic);

    let clearcoatViewFresnel = dielectricSchlick(
      0.04,
      clamp(abs(dot(clearcoatNormal, v)), 0.0, 1.0)
    );
    let sheenDirectionalAlbedo = maxComponent(sheenColor) *
      (0.157 + 0.343 * (1.0 - NdotV)) * (1.0 - sheenRoughness * 0.5);
    let baseLayerEnergy = (1.0 - clearcoatFactor * clearcoatViewFresnel) *
      (1.0 - clamp(sheenDirectionalAlbedo, 0.0, 1.0));
    diffuseColor *= baseLayerEnergy;
    specularColor *= baseLayerEnergy;

    // Compute reflectance.
    let reflectance = max(max(specularColor.r, specularColor.g), specularColor.b);

    // For typical incident reflectance range (between 4% to 100%) set the grazing
    // reflectance to 100% for typical fresnel effect.
    // For very low reflectance range on highly diffuse objects (below 4%),
    // incrementally reduce grazing reflectance to 0%.
    let reflectance90 = clamp(reflectance * 25.0, 0.0, 1.0);
    let specularEnvironmentR0 = specularColor;
    let specularEnvironmentR90 = vec3<f32>(1.0, 1.0, 1.0) * reflectance90;
    let reflection = -normalize(reflect(v, n));

    var pbrInfo = PBRInfo(
      0.0, // NdotL
      NdotV,
      0.0, // NdotH
      0.0, // LdotH
      0.0, // VdotH
      perceptualRoughness,
      metallic,
      specularEnvironmentR0,
      specularEnvironmentR90,
      alphaRoughness,
      diffuseColor,
      specularColor,
      n,
      v,
      n,
      n
    );

    #ifdef USE_LIGHTS
    // Apply ambient light
    PBRInfo_setAmbientLight(&pbrInfo);
    color += calculateMaterialLightColor(
      pbrInfo,
      lighting.ambientColor,
      clearcoatNormal,
      clearcoatFactor,
      clearcoatRoughness,
      sheenColor,
      sheenRoughness,
      anisotropyTangent,
      anisotropyStrength
    );

    // Apply directional light
    for (var i = 0; i < lighting.directionalLightCount; i++) {
      if (i < lighting.directionalLightCount) {
        PBRInfo_setDirectionalLight(&pbrInfo, lighting_getDirectionalLight(i).direction);
        color += calculateMaterialLightColor(
          pbrInfo,
          lighting_getDirectionalLight(i).color,
          clearcoatNormal,
          clearcoatFactor,
          clearcoatRoughness,
          sheenColor,
          sheenRoughness,
          anisotropyTangent,
          anisotropyStrength
        );
        color += calculateDiffuseTransmissionLight(
          pbrInfo,
          lighting_getDirectionalLight(i).color,
          diffuseTransmissionColor,
          diffuseTransmission,
          multiscatterColor,
          thickness
        );
      }
    }

    // Apply point light
    for (var i = 0; i < lighting.pointLightCount; i++) {
      if (i < lighting.pointLightCount) {
        PBRInfo_setPointLight(&pbrInfo, lighting_getPointLight(i));
        let attenuation = getPointLightAttenuation(
          lighting_getPointLight(i),
          distance(lighting_getPointLight(i).position, fragmentInputs.pbr_vPosition)
        );
        color += calculateMaterialLightColor(
          pbrInfo,
          lighting_getPointLight(i).color / attenuation,
          clearcoatNormal,
          clearcoatFactor,
          clearcoatRoughness,
          sheenColor,
          sheenRoughness,
          anisotropyTangent,
          anisotropyStrength
        );
        color += calculateDiffuseTransmissionLight(
          pbrInfo,
          lighting_getPointLight(i).color / attenuation,
          diffuseTransmissionColor,
          diffuseTransmission,
          multiscatterColor,
          thickness
        );
      }
    }

    for (var i = 0; i < lighting.spotLightCount; i++) {
      if (i < lighting.spotLightCount) {
        PBRInfo_setSpotLight(&pbrInfo, lighting_getSpotLight(i));
        let attenuation = getSpotLightAttenuation(lighting_getSpotLight(i), fragmentInputs.pbr_vPosition);
        color += calculateMaterialLightColor(
          pbrInfo,
          lighting_getSpotLight(i).color / attenuation,
          clearcoatNormal,
          clearcoatFactor,
          clearcoatRoughness,
          sheenColor,
          sheenRoughness,
          anisotropyTangent,
          anisotropyStrength
        );
        color += calculateDiffuseTransmissionLight(
          pbrInfo,
          lighting_getSpotLight(i).color / attenuation,
          diffuseTransmissionColor,
          diffuseTransmission,
          multiscatterColor,
          thickness
        );
      }
    }
    #endif

    // Calculate lighting contribution from image based lighting source (IBL)
    #ifdef USE_IBL
    if (pbrMaterial.IBLenabled != 0) {
      color += getIBLContribution(
        pbrInfo,
        n,
        getAnisotropicReflection(pbrInfo, anisotropyTangent, anisotropyStrength)
      );
      color += calculateClearcoatIBLContribution(
        pbrInfo,
        clearcoatNormal,
        -normalize(reflect(v, clearcoatNormal)),
        clearcoatFactor,
        clearcoatRoughness
      );
      color += calculateDiffuseTransmissionIBL(
        pbrInfo,
        diffuseTransmissionColor,
        diffuseTransmission,
        multiscatterColor,
        thickness
      );
      color += sheenColor * pbrMaterial.scaleIBLAmbient.x * (1.0 - sheenRoughness) * 0.25;
    }
    #endif

    // Apply optional PBR terms for additional (optional) shading
    #ifdef HAS_OCCLUSIONMAP
    if (pbrMaterial.occlusionMapEnabled != 0) {
      let ao = textureSample(pbr_occlusionSampler, pbr_occlusionSamplerSampler, occlusionUV).r;
      color = mix(color, color * ao, pbrMaterial.occlusionStrength);
    }
    #endif

    var emissive = pbrMaterial.emissiveFactor;
    #ifdef HAS_EMISSIVEMAP
    if (pbrMaterial.emissiveMapEnabled != 0u) {
      emissive *= SRGBtoLINEAR(
        textureSample(pbr_emissiveSampler, pbr_emissiveSamplerSampler, emissiveUV)
      ).rgb;
    }
    #endif
    color += emissive * pbrMaterial.emissiveStrength;

    if (transmission > 0.0) {
      #ifdef USE_TRANSMISSION_FRAMEBUFFER
      let dielectricFresnel = getDielectricF0(pbrMaterial.ior);
      let transmissionFresnel = dielectricFresnel +
        (1.0 - dielectricFresnel) * pow(1.0 - NdotV, 5.0);
      let transmittedColor = getTransmittedSceneColor(
        fragmentInputs.pbr_vPosition,
        n,
        v,
        thickness,
        perceptualRoughness
      );
      color += transmittedColor * getVolumeAttenuation(thickness) *
        transmission * (1.0 - transmissionFresnel);
      #else
      color = mix(color, color * getVolumeAttenuation(thickness), transmission);
      #endif
    }

    // This section uses mix to override final color for reference app visualization
    // of various parameters in the lighting equation.
    #ifdef PBR_DEBUG
    // TODO: Figure out how to debug multiple lights

    // color = mix(color, F, pbr_scaleFGDSpec.x);
    // color = mix(color, vec3(G), pbr_scaleFGDSpec.y);
    // color = mix(color, vec3(D), pbr_scaleFGDSpec.z);
    // color = mix(color, specContrib, pbr_scaleFGDSpec.w);

    // color = mix(color, diffuseContrib, pbr_scaleDiffBaseMR.x);
    color = mix(color, baseColor.rgb, pbrMaterial.scaleDiffBaseMR.y);
    color = mix(color, vec3<f32>(metallic), pbrMaterial.scaleDiffBaseMR.z);
    color = mix(color, vec3<f32>(perceptualRoughness), pbrMaterial.scaleDiffBaseMR.w);
    #endif
  }

  #ifdef USE_TRANSMISSION_FRAMEBUFFER
  let alpha = clamp(baseColor.a, 0.0, 1.0);
  #else
  let alpha = clamp(baseColor.a * (1.0 - transmission), 0.0, 1.0);
  #endif
  return vec4<f32>(applySceneColorManagement(color), alpha);
}
`,Zr=`layout(std140) uniform pbrProjectionUniforms {
  mat4 modelViewProjectionMatrix;
  mat4 modelMatrix;
  mat4 normalMatrix;
  vec3 camera;
} pbrProjection;
`,Qr={props:{},uniforms:{},defaultUniforms:{unlit:!1,baseColorMapEnabled:!1,baseColorFactor:[1,1,1,1],normalMapEnabled:!1,normalScale:1,emissiveMapEnabled:!1,emissiveFactor:[0,0,0],metallicRoughnessValues:[1,1],metallicRoughnessMapEnabled:!1,occlusionMapEnabled:!1,occlusionStrength:1,alphaCutoffEnabled:!1,alphaCutoff:.5,IBLenabled:!1,scaleIBLAmbient:[1,1],scaleDiffBaseMR:[0,0,0,0],scaleFGDSpec:[0,0,0,0],specularColorFactor:[1,1,1],specularIntensityFactor:1,specularColorMapEnabled:!1,specularIntensityMapEnabled:!1,ior:1.5,transmissionFactor:0,transmissionMapEnabled:!1,thicknessFactor:0,attenuationDistance:1e9,attenuationColor:[1,1,1],clearcoatFactor:0,clearcoatRoughnessFactor:0,clearcoatMapEnabled:!1,clearcoatRoughnessMapEnabled:!1,sheenColorFactor:[0,0,0],sheenRoughnessFactor:0,sheenColorMapEnabled:!1,sheenRoughnessMapEnabled:!1,iridescenceFactor:0,iridescenceIor:1.3,iridescenceThicknessRange:[100,400],iridescenceMapEnabled:!1,anisotropyStrength:0,anisotropyRotation:0,anisotropyDirection:[1,0],anisotropyMapEnabled:!1,emissiveStrength:1,dispersion:0,baseColorUVSet:0,baseColorUVTransform:[1,0,0,0,1,0,0,0,1],metallicRoughnessUVSet:0,metallicRoughnessUVTransform:[1,0,0,0,1,0,0,0,1],normalUVSet:0,normalUVTransform:[1,0,0,0,1,0,0,0,1],occlusionUVSet:0,occlusionUVTransform:[1,0,0,0,1,0,0,0,1],emissiveUVSet:0,emissiveUVTransform:[1,0,0,0,1,0,0,0,1],specularColorUVSet:0,specularColorUVTransform:[1,0,0,0,1,0,0,0,1],specularIntensityUVSet:0,specularIntensityUVTransform:[1,0,0,0,1,0,0,0,1],transmissionUVSet:0,transmissionUVTransform:[1,0,0,0,1,0,0,0,1],thicknessUVSet:0,thicknessUVTransform:[1,0,0,0,1,0,0,0,1],clearcoatUVSet:0,clearcoatUVTransform:[1,0,0,0,1,0,0,0,1],clearcoatRoughnessUVSet:0,clearcoatRoughnessUVTransform:[1,0,0,0,1,0,0,0,1],clearcoatNormalUVSet:0,clearcoatNormalUVTransform:[1,0,0,0,1,0,0,0,1],sheenColorUVSet:0,sheenColorUVTransform:[1,0,0,0,1,0,0,0,1],sheenRoughnessUVSet:0,sheenRoughnessUVTransform:[1,0,0,0,1,0,0,0,1],iridescenceUVSet:0,iridescenceUVTransform:[1,0,0,0,1,0,0,0,1],iridescenceThicknessUVSet:0,iridescenceThicknessUVTransform:[1,0,0,0,1,0,0,0,1],anisotropyUVSet:0,anisotropyUVTransform:[1,0,0,0,1,0,0,0,1],bumpFactor:1,bumpMapEnabled:!1,diffuseTransmissionFactor:0,diffuseTransmissionMapEnabled:!1,diffuseTransmissionColorFactor:[1,1,1],diffuseTransmissionColorMapEnabled:!1,multiscatterColorFactor:[0,0,0],multiscatterColorMapEnabled:!1,scatterAnisotropy:0,bumpUVSet:0,bumpUVTransform:[1,0,0,0,1,0,0,0,1],diffuseTransmissionUVSet:0,diffuseTransmissionUVTransform:[1,0,0,0,1,0,0,0,1],diffuseTransmissionColorUVSet:0,diffuseTransmissionColorUVTransform:[1,0,0,0,1,0,0,0,1],multiscatterColorUVSet:0,multiscatterColorUVTransform:[1,0,0,0,1,0,0,0,1]},name:`pbrMaterial`,firstBindingSlot:0,bindingLayout:[{name:`pbrMaterial`,group:3},{name:`pbr_baseColorSampler`,group:3},{name:`pbr_normalSampler`,group:3},{name:`pbr_emissiveSampler`,group:3},{name:`pbr_metallicRoughnessSampler`,group:3},{name:`pbr_occlusionSampler`,group:3},{name:`pbr_specularColorSampler`,group:3},{name:`pbr_specularIntensitySampler`,group:3},{name:`pbr_transmissionSampler`,group:3},{name:`pbr_thicknessSampler`,group:3},{name:`pbr_clearcoatSampler`,group:3},{name:`pbr_clearcoatRoughnessSampler`,group:3},{name:`pbr_clearcoatNormalSampler`,group:3},{name:`pbr_sheenColorSampler`,group:3},{name:`pbr_sheenRoughnessSampler`,group:3},{name:`pbr_iridescenceSampler`,group:3},{name:`pbr_iridescenceThicknessSampler`,group:3},{name:`pbr_anisotropySampler`,group:3},{name:`pbr_bumpSampler`,group:3},{name:`pbr_diffuseTransmissionSampler`,group:3},{name:`pbr_diffuseTransmissionColorSampler`,group:3},{name:`pbr_multiscatterColorSampler`,group:3}],dependencies:[jr,Hr,{name:`pbrProjection`,bindingLayout:[{name:`pbrProjection`,group:0}],source:`struct pbrProjectionUniforms {
  modelViewProjectionMatrix: mat4x4<f32>,
  modelMatrix: mat4x4<f32>,
  normalMatrix: mat4x4<f32>,
  camera: vec3<f32>
};

@group(0) @binding(auto) var<uniform> pbrProjection: pbrProjectionUniforms;
`,vs:Zr,fs:Zr,getUniforms:e=>e,uniformTypes:{modelViewProjectionMatrix:`mat4x4<f32>`,modelMatrix:`mat4x4<f32>`,normalMatrix:`mat4x4<f32>`,camera:`vec3<f32>`}}],source:Xr,vs:Jr,fs:Yr,defines:{LIGHTING_FRAGMENT:!0,HAS_NORMALMAP:!1,HAS_EMISSIVEMAP:!1,HAS_OCCLUSIONMAP:!1,HAS_BASECOLORMAP:!1,HAS_METALROUGHNESSMAP:!1,HAS_SPECULARCOLORMAP:!1,HAS_SPECULARINTENSITYMAP:!1,HAS_TRANSMISSIONMAP:!1,HAS_THICKNESSMAP:!1,HAS_CLEARCOATMAP:!1,HAS_CLEARCOATROUGHNESSMAP:!1,HAS_CLEARCOATNORMALMAP:!1,HAS_SHEENCOLORMAP:!1,HAS_SHEENROUGHNESSMAP:!1,HAS_IRIDESCENCEMAP:!1,HAS_IRIDESCENCETHICKNESSMAP:!1,HAS_ANISOTROPYMAP:!1,HAS_BUMPMAP:!1,HAS_DIFFUSETRANSMISSIONMAP:!1,HAS_DIFFUSETRANSMISSIONCOLORMAP:!1,HAS_MULTISCATTERCOLORMAP:!1,USE_MATERIAL_EXTENSIONS:!1,ALPHA_CUTOFF:!1,USE_IBL:!1,PBR_DEBUG:!1},getUniforms:e=>e,uniformTypes:{unlit:`i32`,baseColorMapEnabled:`i32`,baseColorFactor:`vec4<f32>`,normalMapEnabled:`i32`,normalScale:`f32`,emissiveMapEnabled:`i32`,emissiveFactor:`vec3<f32>`,metallicRoughnessValues:`vec2<f32>`,metallicRoughnessMapEnabled:`i32`,occlusionMapEnabled:`i32`,occlusionStrength:`f32`,alphaCutoffEnabled:`i32`,alphaCutoff:`f32`,specularColorFactor:`vec3<f32>`,specularIntensityFactor:`f32`,specularColorMapEnabled:`i32`,specularIntensityMapEnabled:`i32`,ior:`f32`,transmissionFactor:`f32`,transmissionMapEnabled:`i32`,thicknessFactor:`f32`,attenuationDistance:`f32`,attenuationColor:`vec3<f32>`,clearcoatFactor:`f32`,clearcoatRoughnessFactor:`f32`,clearcoatMapEnabled:`i32`,clearcoatRoughnessMapEnabled:`i32`,sheenColorFactor:`vec3<f32>`,sheenRoughnessFactor:`f32`,sheenColorMapEnabled:`i32`,sheenRoughnessMapEnabled:`i32`,iridescenceFactor:`f32`,iridescenceIor:`f32`,iridescenceThicknessRange:`vec2<f32>`,iridescenceMapEnabled:`i32`,anisotropyStrength:`f32`,anisotropyRotation:`f32`,anisotropyDirection:`vec2<f32>`,anisotropyMapEnabled:`i32`,emissiveStrength:`f32`,dispersion:`f32`,IBLenabled:`i32`,scaleIBLAmbient:`vec2<f32>`,scaleDiffBaseMR:`vec4<f32>`,scaleFGDSpec:`vec4<f32>`,baseColorUVSet:`i32`,baseColorUVTransform:`mat3x3<f32>`,metallicRoughnessUVSet:`i32`,metallicRoughnessUVTransform:`mat3x3<f32>`,normalUVSet:`i32`,normalUVTransform:`mat3x3<f32>`,occlusionUVSet:`i32`,occlusionUVTransform:`mat3x3<f32>`,emissiveUVSet:`i32`,emissiveUVTransform:`mat3x3<f32>`,specularColorUVSet:`i32`,specularColorUVTransform:`mat3x3<f32>`,specularIntensityUVSet:`i32`,specularIntensityUVTransform:`mat3x3<f32>`,transmissionUVSet:`i32`,transmissionUVTransform:`mat3x3<f32>`,thicknessUVSet:`i32`,thicknessUVTransform:`mat3x3<f32>`,clearcoatUVSet:`i32`,clearcoatUVTransform:`mat3x3<f32>`,clearcoatRoughnessUVSet:`i32`,clearcoatRoughnessUVTransform:`mat3x3<f32>`,clearcoatNormalUVSet:`i32`,clearcoatNormalUVTransform:`mat3x3<f32>`,sheenColorUVSet:`i32`,sheenColorUVTransform:`mat3x3<f32>`,sheenRoughnessUVSet:`i32`,sheenRoughnessUVTransform:`mat3x3<f32>`,iridescenceUVSet:`i32`,iridescenceUVTransform:`mat3x3<f32>`,iridescenceThicknessUVSet:`i32`,iridescenceThicknessUVTransform:`mat3x3<f32>`,anisotropyUVSet:`i32`,anisotropyUVTransform:`mat3x3<f32>`,bumpFactor:`f32`,bumpMapEnabled:`i32`,diffuseTransmissionFactor:`f32`,diffuseTransmissionMapEnabled:`i32`,diffuseTransmissionColorFactor:`vec3<f32>`,diffuseTransmissionColorMapEnabled:`i32`,multiscatterColorFactor:`vec3<f32>`,multiscatterColorMapEnabled:`i32`,scatterAnisotropy:`f32`,bumpUVSet:`i32`,bumpUVTransform:`mat3x3<f32>`,diffuseTransmissionUVSet:`i32`,diffuseTransmissionUVTransform:`mat3x3<f32>`,diffuseTransmissionColorUVSet:`i32`,diffuseTransmissionColorUVTransform:`mat3x3<f32>`,multiscatterColorUVSet:`i32`,multiscatterColorUVTransform:`mat3x3<f32>`}},$r=class{name;playing=!0;speed=1;startTime=0;constructor(e={}){this.name=e.name||`unnamed`,Object.assign(this,e)}setTime(e){if(!this.playing)return;let t=(e/1e3-this.startTime)*this.speed;this.applyTime(t)}},ei=class{clips;animations;constructor(e){this.clips=e,this.animations=e}animate(e){w.warn(`${this.constructor.name}#animate is deprecated. Use ${this.constructor.name}#setTime instead`)(),this.setTime(e)}setTime(e){this.clips.forEach(t=>t.setTime(e))}getAnimations(){return this.clips}};function ti(e,t,n=`vector`){let{input:r,output:i,interpolation:a=`LINEAR`}=t;if(!r.length||!i.length||!Number.isFinite(e))return null;let o=r.length-1;if(e<=r[0]||o===0)return ri(i,a,0,n);if(e>=r[o])return ri(i,a,o,n);let s=0,c=o;for(;c-s>1;){let t=Math.floor((s+c)/2);r[t]<=e?s=t:c=t}let l=r[s],u=r[c]-l;if(u<=0||a===`STEP`)return ri(i,a,s,n);let d=(e-l)/u;switch(a){case`LINEAR`:{let e=i[s],t=i[c];return!e||!t?null:n===`quaternion`?ni(e,t,d):ii(e,t,d)}case`CUBICSPLINE`:{let e=i[s*3+1],t=i[s*3+2],r=i[c*3],a=i[c*3+1];if(!e||!t||!r||!a)return null;let o=ai(e,t,r,a,u,d);return n===`quaternion`?oi(o):o}default:return null}}function ni(e,t,n){let r=oi(e),i=oi(t),a=r.reduce((e,t,n)=>e+t*i[n],0),o=a<0?-1:1;if(a=Math.min(Math.abs(a),1),a>.9995)return oi(r.map((e,t)=>e+n*(i[t]*o-e)));let s=Math.acos(a),c=Math.sin(s),l=Math.sin((1-n)*s)/c,u=Math.sin(n*s)/c*o;return oi(r.map((e,t)=>e*l+i[t]*u))}function ri(e,t,n,r){let i=e[t===`CUBICSPLINE`?n*3+1:n];return i?r===`quaternion`?oi(i):[...i]:null}function ii(e,t,n){return e.map((e,r)=>(1-n)*e+n*t[r])}function ai(e,t,n,r,i,a){let o=a*a,s=o*a;return e.map((e,c)=>(2*s-3*o+1)*e+(s-2*o+a)*t[c]*i+(-2*s+3*o)*r[c]+(s-o)*n[c]*i)}function oi(e){let t=Math.hypot(...e);return t>0?e.map(e=>e/t):[0,0,0,1]}var si=class{name;times;values;interpolation;valueType;binding;constructor(e){this.name=e.name||e.binding.id||`unnamed`,this.times=e.times,this.values=e.values,this.interpolation=e.interpolation||`LINEAR`,this.valueType=e.valueType||`vector`,this.binding=e.binding}get duration(){return this.times[this.times.length-1]||0}get sampler(){return{input:this.times,output:this.values,interpolation:this.interpolation}}evaluate(e){return ti(e,this.sampler,this.valueType)}},ci=class{name;tracks;duration;constructor(e){this.name=e.name||`unnamed`,this.tracks=e.tracks,this.duration=e.duration??Math.max(0,...e.tracks.map(e=>e.duration))}},li=class{clip;mixer;time=0;timeScale;weight;loop;repetitions;paused=!1;playing=!1;elapsedTime=0;fade=null;constructor(e,t,n={}){this.mixer=e,this.clip=t,this.loop=n.loop||`repeat`,this.repetitions=n.repetitions??1/0,this.timeScale=n.timeScale??1,this.weight=n.weight??1}play(){return this.playing=!0,this.paused=!1,this}pause(){return this.paused=!0,this}resume(){return this.playing=!0,this.paused=!1,this}stop(){return this.playing=!1,this.paused=!1,this.fade=null,this.reset()}reset(){return this.elapsedTime=0,this.time=0,this}setTime(e){return this.elapsedTime=e,this.time=this.resolveLocalTime(e),this}setLoop(e,t=1/0){return this.loop=e,this.repetitions=t,this.time=this.resolveLocalTime(this.elapsedTime),this}setEffectiveWeight(e){return this.weight=Math.max(0,e),this.fade=null,this}setEffectiveTimeScale(e){return this.timeScale=e,this}fadeIn(e){return this.scheduleFade(1,e)}fadeOut(e){return this.scheduleFade(0,e)}crossFadeTo(e,t){return e.weight=0,e.play().fadeIn(t),this.fadeOut(t)}crossFadeFrom(e,t){return e.crossFadeTo(this,t),this}advance(e){!this.playing||this.paused||(this.advanceFade(Math.abs(e)),this.elapsedTime+=e*this.timeScale,this.time=this.resolveLocalTime(this.elapsedTime),this.hasFinished()&&(this.playing=!1))}get shouldApply(){return(this.playing||this.hasFinished())&&this.weight>0}scheduleFade(e,t){return t<=0?(this.weight=e,this.fade=null,this):(this.fade={duration:t,elapsedTime:0,startWeight:this.weight,endWeight:e},this)}advanceFade(e){if(!this.fade)return;this.fade.elapsedTime+=e;let t=Math.min(this.fade.elapsedTime/this.fade.duration,1);this.weight=this.fade.startWeight+(this.fade.endWeight-this.fade.startWeight)*t,t===1&&(this.fade=null)}hasFinished(){let e=this.clip.duration;return e<=0?this.loop===`once`:this.loop===`once`?this.elapsedTime>=e||this.elapsedTime<0:Number.isFinite(this.repetitions)&&Math.abs(this.elapsedTime)>=e*this.repetitions}resolveLocalTime(e){let t=this.clip.duration;if(t<=0)return 0;if(this.loop===`once`)return Math.min(Math.max(e,0),t);if(Number.isFinite(this.repetitions)&&Math.abs(e)>=t*this.repetitions)return this.loop===`ping-pong`&&this.repetitions%2==0||e<0?0:t;let n=e>=0&&e<t?e:(e%t+t)%t;if(this.loop===`repeat`)return n;let r=Math.floor(e/t);return Math.abs(r%2)===0?n:t-n}},ui=class{time=0;timeScale=1;clips=new Map;actions=new Map;initialValues=new Map;constructor(e=[]){e.forEach(e=>this.addClip(e))}addClip(e){return this.clips.set(e.name,e),this}clipAction(e,t){let n=typeof e==`string`?this.clips.get(e):e;if(!n)throw Error(`Unknown animation clip: ${e}`);this.addClip(n);let r=this.actions.get(n);return r||(r=new li(this,n,t),this.actions.set(n,r)),r}getAction(e){let t=this.clips.get(e);return t?this.actions.get(t):void 0}update(e){return this.advance(e),this.applyValues(),this}advance(e){let t=e*this.timeScale;return this.time+=t,this.actions.forEach(e=>e.advance(t)),this}setTime(e){return this.time=e,this.actions.forEach(t=>{t.paused||t.setTime(e*t.timeScale)}),this.applyValues(),this}stopAllAction(){return this.actions.forEach(e=>e.stop()),this}applyValues(){let e=new Map;this.actions.forEach(t=>{!t.shouldApply&&!(t.playing&&t.weight===0)||t.clip.tracks.forEach(n=>{let r=n.evaluate(t.time);if(!r)return;let i=n.binding.id||n.binding;if(!this.initialValues.has(i)){let e=n.binding.getValue?.();e&&this.initialValues.set(i,[...e])}if(t.weight===0&&!this.initialValues.has(i))return;let a=e.get(i);if(!a){e.set(i,{binding:n.binding,value:[...r],valueType:n.valueType,weight:t.weight});return}if(t.weight===0)return;let o=a.weight+t.weight,s=t.weight/o;a.value=n.valueType===`quaternion`?ni(a.value,r,s):a.value.map((e,t)=>e+(r[t]-e)*s),a.weight=o})}),e.forEach(({binding:e,value:t,valueType:n,weight:r},i)=>{let a=r<1?this.initialValues.get(i):void 0;a&&a.length===t.length&&(t=n===`quaternion`?ni(a,t,r):t.map((e,t)=>a[t]+(e-a[t])*r)),e.setValue(t)})}};function di(e){let t=e.value;if(t instanceof Float32Array)return t;let n=new Float32Array(t.length),r=hi(t),i=t instanceof Int8Array||t instanceof Int16Array||t instanceof Int32Array;for(let a=0;a<t.length;a++){let o=Number(t[a]);n[a]=e.normalized&&r?i?Math.max(o/r,-1):o/r:o}return n}function fi(e,t,n){let r={};for(let i of[`POSITION`,`NORMAL`,`TANGENT`]){let a=e[i];if(!a)continue;let o=new Float32Array(a),s=i===`TANGENT`?4:3,c=Math.floor(a.length/s);for(let e=0;e<Math.min(t.length,n.length);e++){let r=n[e],a=t[e][i];if(!r||!a)continue;let l=i===`TANGENT`&&a.length===c*4?4:3;for(let e=0;e<c;e++){let t=e*s,n=e*l;for(let e=0;e<3;e++)o[t+e]+=(a[n+e]||0)*r}}i!==`POSITION`&&gi(o,s),r[i]=o}return r}function pi(e,t,n,r){let i={};for(let e of[`POSITION`,`NORMAL`,`TANGENT`]){let n=t.attributes[e];n&&(i[e]=di(n))}let a=fi(i,n,r),o={};for(let[e,n]of Object.entries(t.attributes))n&&(o[e]=n);for(let e of[`POSITION`,`NORMAL`,`TANGENT`]){let t=a[e],n=o[e];t&&n&&(o[e]={...n,value:mi(n,t)})}let s=ce(new de({id:t.id,topology:t.topology||`triangle-list`,vertexCount:t.vertexCount,indices:t.indices,attributes:o,bufferLayout:t.bufferLayout})).attributes.geometry?.value,c=e._gpuGeometry?.attributes.geometry||e.bufferAttributes.geometry;if(s&&c){c.write(s);return}for(let t of[`POSITION`,`NORMAL`,`TANGENT`]){let n=a[t];if(n){let r=t===`POSITION`?`positions`:t===`NORMAL`?`normals`:`TANGENT`;e.bufferAttributes[r]?.write(n)}}}function mi(e,t){if(e.value instanceof Float32Array)return t;let n=e.value.slice(),r=hi(n),i=n instanceof Int8Array||n instanceof Int16Array||n instanceof Int32Array;for(let a=0;a<t.length;a++){let o=t[a];n[a]=e.normalized&&r?Math.round(Math.max(i?-1:0,Math.min(1,o))*r):o}return n}function hi(e){return e instanceof Int8Array?127:e instanceof Uint8Array||e instanceof Uint8ClampedArray?255:e instanceof Int16Array?32767:e instanceof Uint16Array?65535:e instanceof Int32Array?2147483647:e instanceof Uint32Array?4294967295:0}function gi(e,t){for(let n=0;n<e.length;n+=t){let t=Math.hypot(e[n],e[n+1],e[n+2]);t>0&&(e[n]/=t,e[n+1]/=t,e[n+2]/=t)}}function _i(e){let{joints:t,meshNode:n,worldMatrices:r,inverseBindMatrices:i,target:a}=e,o=t.length,s=a&&a.length===o*16?a:new Float32Array(o*16),c=n?r.get(n)||n.matrix:void 0,l=c?new R(c).invert():null;for(let e=0;e<o;e++){let n=t[e],a=r.get(n)||n.matrix,o=l?new R(l).multiplyRight(a):new R(a),c=e*16;if(i&&i.length>=c+16){let e=new R;for(let t=0;t<16;t++)e[t]=i[c+t];o.multiplyRight(e)}s.set(o,c)}return s}var vi=1,yi=1,bi=class{time=0;channels=new Map;animations=new Map;playing=!1;lastEngineTime=-1;constructor(){}addChannel(e){let{delay:t=0,duration:n=1/0,rate:r=1,repeat:i=1}=e,a=vi++,o={time:0,delay:t,duration:n,rate:r,repeat:i};return this._setChannelTime(o,this.time),this.channels.set(a,o),a}removeChannel(e){this.channels.delete(e);for(let[t,n]of this.animations)n.channel===e&&this.detachAnimation(t)}isFinished(e){let t=this.channels.get(e);return t!==void 0&&this.time>=t.delay+t.duration*t.repeat}getTime(e){if(e===void 0)return this.time;let t=this.channels.get(e);return t===void 0?-1:t.time}setTime(e){this.time=Math.max(0,e);let t=this.channels.values();for(let e of t)this._setChannelTime(e,this.time);let n=this.animations.values();for(let e of n){let{animation:t,channel:n}=e;t.setTime(this.getTime(n))}}play(){this.playing=!0}pause(){this.playing=!1,this.lastEngineTime=-1}reset(){this.setTime(0)}attachAnimation(e,t){let n=yi++;return this.animations.set(n,{animation:e,channel:t}),e.setTime(this.getTime(t)),n}detachAnimation(e){this.animations.delete(e)}update(e){this.playing&&(this.lastEngineTime===-1&&(this.lastEngineTime=e),this.setTime(this.time+(e-this.lastEngineTime)),this.lastEngineTime=e)}_setChannelTime(e,t){let n=t-e.delay;n>=e.duration*e.repeat?e.time=e.duration*e.rate:(e.time=Math.max(0,n)%e.duration,e.time*=e.rate)}};function xi(e){let t=typeof window<`u`?window.requestAnimationFrame||window.webkitRequestAnimationFrame||window.mozRequestAnimationFrame:null;return t?t.call(window,e):setTimeout(()=>e(typeof performance<`u`?performance.now():Date.now()),1e3/60)}function Si(e){let t=typeof window<`u`?window.cancelAnimationFrame||window.webkitCancelAnimationFrame||window.mozCancelAnimationFrame:null;if(t){t.call(window,e);return}clearTimeout(e)}var Ci=0,wi=`Animation Loop`,Ti={requestAnimationFrame:e=>xi(e),cancelAnimationFrame:e=>Si(e)},Ei=class e{static defaultAnimationLoopProps={device:null,onAddHTML:()=>``,onInitialize:async()=>null,onRender:()=>{},onFinalize:()=>{},onError:e=>{console.error(e)},stats:void 0,autoResizeViewport:!1,animationFrameProvider:Ti};device=null;canvas=null;props;animationProps=null;timeline=null;stats;sharedStats;cpuTime;gpuTime;frameRate;display;_needsRedraw=`initialized`;_initialized=!1;_running=!1;_animationFrameId=null;_nextFramePromise=null;_resolveNextFrame=null;_cpuStartTime=0;_error=null;_lastFrameTime=0;constructor(t){if(this.props={...e.defaultAnimationLoopProps,...t},t=this.props,!t.device)throw Error(`No device provided`);this.stats=t.stats||new j({id:`animation-loop-${Ci++}`}),this.sharedStats=ye.stats.get(wi),this.frameRate=this.stats.get(`Frame Rate`),this.frameRate.setSampleSize(1),this.cpuTime=this.stats.get(`CPU Time`),this.gpuTime=this.stats.get(`GPU Time`),this.setProps({autoResizeViewport:t.autoResizeViewport,animationFrameProvider:t.animationFrameProvider}),this.start=this.start.bind(this),this.stop=this.stop.bind(this),this._onMousemove=this._onMousemove.bind(this),this._onMouseleave=this._onMouseleave.bind(this)}destroy(){this.stop(),this._setDisplay(null),this.device?._disableDebugGPUTime()}delete(){this.destroy()}reportError(t){this._error=t,this.props.onError(t),this.props.onError===e.defaultAnimationLoopProps.onError&&typeof window<`u`&&typeof ErrorEvent<`u`&&window.dispatchEvent(new ErrorEvent(`error`,{error:t,message:t.message}))}setNeedsRedraw(e){return this._needsRedraw=this._needsRedraw||e,this}needsRedraw(){let e=this._needsRedraw;return this._needsRedraw=!1,e}setProps(e){if(`autoResizeViewport`in e&&(this.props.autoResizeViewport=e.autoResizeViewport||!1),`animationFrameProvider`in e){let t=e.animationFrameProvider||Ti;if(t!==this.props.animationFrameProvider){let e=this._animationFrameId!==null;e&&this._cancelAnimationFrame(),this.props.animationFrameProvider=t,e&&this._requestAnimationFrame()}}return this}async start(){if(this._running)return this;this._running=!0;try{if(!this._initialized){if(this._initialized=!0,await this._initDevice(),this._initialize(),!this._running)return null;await this.props.onInitialize(this._getAnimationProps())}return this._running?(this._cancelAnimationFrame(),this._requestAnimationFrame(),this):null}catch(e){let t=e instanceof Error?e:Error(`Unknown error`);throw this.props.onError(t),t}}stop(){if(this._running){let e=this.animationProps;this._cancelAnimationFrame(),this._nextFramePromise=null,this._resolveNextFrame=null,this._running=!1,this._lastFrameTime=0,e&&this.props.onFinalize(e)}return this}redraw(e,t=null){return this.device?.isLost||this._error?this:(this._beginFrameTimers(e),this._setupFrame(),this.animationProps&&(this.animationProps.animationFrame=t),this._updateAnimationProps(),this._renderFrame(this._getAnimationProps()),this._clearNeedsRedraw(),this._resolveNextFrame&&=(this._resolveNextFrame(this),this._nextFramePromise=null,null),this._endFrameTimers(),this)}attachTimeline(e){return this.timeline=e,this.timeline}detachTimeline(){this.timeline=null}waitForRender(){return this.setNeedsRedraw(`waitForRender`),this._nextFramePromise||=new Promise(e=>{this._resolveNextFrame=e}),this._nextFramePromise}async toDataURL(){if(this.setNeedsRedraw(`toDataURL`),await this.waitForRender(),this.canvas instanceof HTMLCanvasElement)return this.canvas.toDataURL();throw Error(`OffscreenCanvas`)}_initialize(){this._startEventHandling(),this._initializeAnimationProps(),this._updateAnimationProps(),this._resizeViewport(),this.device?._enableDebugGPUTime()}_setDisplay(e){this.display&&(this.display.destroy(),this.display.animationLoop=null),e&&(e.animationLoop=this),this.display=e}_requestAnimationFrame(){this._running&&(this._animationFrameId=this.props.animationFrameProvider.requestAnimationFrame(this._animationFrame.bind(this)))}_cancelAnimationFrame(){this._animationFrameId!==null&&(this.props.animationFrameProvider.cancelAnimationFrame(this._animationFrameId),this._animationFrameId=null)}_animationFrame(e,t){if(this._running)try{this.redraw(e,t??null),this._requestAnimationFrame()}catch(e){let t=e instanceof Error?e:Error(String(e));this.reportError(t),this.stop()}}_renderFrame(e){if(this.display){this.display._renderFrame(e);return}let t=this.props.onRender(this._getAnimationProps());this.device&&t!==!1&&this.device.submit()}_clearNeedsRedraw(){this._needsRedraw=!1}_setupFrame(){this._resizeViewport()}_initializeAnimationProps(){let e=this.device?.getDefaultCanvasContext();if(!this.device||!e)throw Error(`loop`);let t=e?.canvas,n=e.props.useDevicePixels;this.animationProps={animationLoop:this,device:this.device,canvasContext:e,canvas:t,useDevicePixels:n,timeline:this.timeline,needsRedraw:!1,width:1,height:1,aspect:1,time:0,startTime:Date.now(),engineTime:0,tick:0,tock:0,animationFrame:null,_mousePosition:null}}_getAnimationProps(){if(!this.animationProps)throw Error(`animationProps`);return this.animationProps}_updateAnimationProps(){if(!this.animationProps)return;let{width:e,height:t,aspect:n}=this._getSizeAndAspect();(e!==this.animationProps.width||t!==this.animationProps.height)&&this.setNeedsRedraw(`drawing buffer resized`),n!==this.animationProps.aspect&&this.setNeedsRedraw(`drawing buffer aspect changed`),this.animationProps.width=e,this.animationProps.height=t,this.animationProps.aspect=n,this.animationProps.needsRedraw=this._needsRedraw,this.animationProps.engineTime=Date.now()-this.animationProps.startTime,this.timeline&&this.timeline.update(this.animationProps.engineTime),this.animationProps.tick=Math.floor(this.animationProps.time/1e3*60),this.animationProps.tock++,this.animationProps.time=this.timeline?this.timeline.getTime():this.animationProps.engineTime}async _initDevice(){if(this.device=await this.props.device,!this.device)throw Error(`No device provided`);this.canvas=this.device.getDefaultCanvasContext().canvas||null}_createInfoDiv(){if(this.canvas&&this.props.onAddHTML){let e=document.createElement(`div`);document.body.appendChild(e),e.style.position=`relative`;let t=document.createElement(`div`);t.style.position=`absolute`,t.style.left=`10px`,t.style.bottom=`10px`,t.style.width=`300px`,t.style.background=`white`,this.canvas instanceof HTMLCanvasElement&&e.appendChild(this.canvas),e.appendChild(t);let n=this.props.onAddHTML(t);n&&(t.innerHTML=n)}}_getSizeAndAspect(){if(!this.device)return{width:1,height:1,aspect:1};let[e,t]=this.device.getDefaultCanvasContext().getDrawingBufferSize();return{width:e,height:t,aspect:e>0&&t>0?e/t:1}}_resizeViewport(){this.props.autoResizeViewport&&this.device.gl&&this.device.gl.viewport(0,0,this.device.gl.drawingBufferWidth,this.device.gl.drawingBufferHeight)}_beginFrameTimers(e){let t=e??(typeof performance<`u`?performance.now():Date.now());if(this._lastFrameTime){let e=t-this._lastFrameTime;e>0&&this.frameRate.addTime(e)}this._lastFrameTime=t,this.device?._isDebugGPUTimeEnabled()&&this._consumeEncodedGpuTime(),this.cpuTime.timeStart()}_endFrameTimers(){this.device?._isDebugGPUTimeEnabled()&&this._consumeEncodedGpuTime(),this.cpuTime.timeEnd(),this._updateSharedStats()}_consumeEncodedGpuTime(){if(!this.device)return;let e=this.device.commandEncoder._gpuTimeMs;e!==void 0&&(this.gpuTime.addTime(e),this.device.commandEncoder._gpuTimeMs=void 0)}_updateSharedStats(){if(this.stats!==this.sharedStats){for(let e of Object.keys(this.sharedStats.stats))this.stats.stats[e]||delete this.sharedStats.stats[e];this.stats.forEach(e=>{let t=this.sharedStats.get(e.name,e.type);t.sampleSize=e.sampleSize,t.time=e.time,t.count=e.count,t.samples=e.samples,t.lastTiming=e.lastTiming,t.lastSampleTime=e.lastSampleTime,t.lastSampleCount=e.lastSampleCount,t._count=e._count,t._time=e._time,t._samples=e._samples,t._startTime=e._startTime,t._timerPending=e._timerPending})}}_startEventHandling(){this.canvas&&(this.canvas.addEventListener(`mousemove`,this._onMousemove.bind(this)),this.canvas.addEventListener(`mouseleave`,this._onMouseleave.bind(this)))}_onMousemove(e){e instanceof MouseEvent&&(this._getAnimationProps()._mousePosition=[e.offsetX,e.offsetY])}_onMouseleave(e){this._getAnimationProps()._mousePosition=null}},Di=class{device;modules;_materialBindingNames;_materialModuleNames;constructor(e,t={}){this.device=e,this.modules=t.modules||[];let n=new h(Object.fromEntries(this.modules.map(e=>[e.name,e])));this._materialBindingNames=ki(n),this._materialModuleNames=Ai(n)}createMaterial(e={}){return new ji(this.device,{...e,factory:this})}getBindingNames(){return Array.from(this._materialBindingNames)}ownsBinding(e){if(this._materialBindingNames.has(e))return!0;let t=Oi(e);return t?this._materialModuleNames.has(t):!1}ownsModule(e){return this._materialModuleNames.has(e)}getBindingsByGroup(e){return Object.keys(e).length>0?{3:e}:{}}};function Oi(e){return e.endsWith(`Uniforms`)?e.slice(0,-8):null}function ki(e){let t=new Set;for(let n of Object.values(e.modules))for(let e of n.bindingLayout||[])e.group===3&&t.add(e.name);return t}function Ai(e){let t=new Set;for(let n of Object.values(e.modules))n.name&&n.bindingLayout?.some(e=>e.group===3&&e.name===n.name)&&t.add(n.name);return t}var ji=class{id;device;factory;shaderInputs;bindings={};_uniformStore;_bindGroupCacheToken={};_dynamicResourceGenerations={};constructor(e,t={}){this.id=t.id||f(`material`),this.device=e,this.factory=t.factory||new Di(e,{modules:t.modules||t.shaderInputs?.getModules()||[]});let n=Object.fromEntries((t.shaderInputs?.getModules()||this.factory.modules).map(e=>[e.name,e]));this.shaderInputs=t.shaderInputs||new h(n),this._uniformStore=new c(this.device,this.shaderInputs.modules);for(let[e,t]of Object.entries(this.shaderInputs.modules))if(this.ownsModule(e)&&p(t)){let t=this._uniformStore.getManagedUniformBuffer(e);this.bindings[`${e}Uniforms`]=t}this.updateShaderInputs(),t.bindings&&this._replaceOwnedBindings(t.bindings)}destroy(){this._uniformStore.destroy()}clone(e={}){let t=this.factory.createMaterial({id:e.id,shaderInputs:e.shaderInputs,bindings:{...this.getResourceBindings(),...e.bindings}});return e.shaderInputs||t.setProps(this.shaderInputs.getUniformValues()),e.moduleProps&&t.setProps(e.moduleProps),t.updateShaderInputs(),t}ownsBinding(e){return this.factory.ownsBinding(e)}ownsModule(e){return this.factory.ownsModule(e)}setProps(e){this.shaderInputs.setProps(e)}updateShaderInputs(e){this._uniformStore.setUniforms(this.shaderInputs.getUniformValues(),e),this._setOwnedBindings(this.shaderInputs.getBindingValues())&&(this._bindGroupCacheToken={})}getResourceBindings(){let e={};for(let[t,n]of Object.entries(this.bindings))Oi(t)||(e[t]=n);return e}getBindings(e={bindings:[]}){this._syncDynamicResourceGenerations();let t={},n=t;for(let[t,r]of Object.entries(this.bindings))if(le(r)){let i=fe(e,t,{fallbackGroup:3}),a=i?r.resolveTextureBinding(i):null;a&&(n[t]=a)}else r instanceof u?n[t]=r.buffer:v(r)?n[t]=d(r):n[t]=r;return this._syncDynamicResourceGenerations(),t}getBindingsByGroup(e={bindings:[]}){return this.factory.getBindingsByGroup(this.getBindings(e))}getBindGroupCacheKey(e){return this._syncDynamicResourceGenerations(),e===3?this._bindGroupCacheToken:null}getBindingsUpdateTimestamp(){let e=0;for(let t of Object.values(this.bindings))t instanceof ae?e=Math.max(e,t.texture.updateTimestamp):t instanceof C||t instanceof M||t instanceof pe||t instanceof u?e=Math.max(e,t.updateTimestamp):le(t)?e=t.isReady?Math.max(e,t.updateTimestamp):1/0:v(t)&&(e=Math.max(e,(t.buffer instanceof u,t.buffer.updateTimestamp)));return e}_replaceOwnedBindings(e){this._setOwnedBindings(e)&&(this._bindGroupCacheToken={})}_setOwnedBindings(e){let t=!1;for(let[n,r]of Object.entries(e))r!==void 0&&this.ownsBinding(n)&&this.bindings[n]!==r&&(this.bindings[n]=r,t=!0);return t}_syncDynamicResourceGenerations(){let e={},t=!1;for(let[n,r]of Object.entries(this.bindings)){let i=Mi(r);i!==null&&(e[n]=i,this._dynamicResourceGenerations[n]!==i&&(t=!0))}Object.keys(e).length!==Object.keys(this._dynamicResourceGenerations).length&&(t=!0),this._dynamicResourceGenerations=e,t&&(this._bindGroupCacheToken={})}};function Mi(e){return le(e)?e.generation:_(e)?.generation??null}var Ni={"+X":0,"-X":1,"+Y":2,"-Y":3,"+Z":4,"-Z":5};function Pi(e){return e?Array.isArray(e)?e[0]??null:e:null}function Fi(e){let{dimension:t,data:n}=e;if(!n)return null;switch(t){case`1d`:{let e=Pi(n);if(!e)return null;let{width:t}=Ii(e);return{width:t,height:1}}case`2d`:{if(ArrayBuffer.isView(n))return null;let e=Pi(n);return e?Ii(e):null}case`3d`:case`2d-array`:{if(!Array.isArray(n)||n.length===0)return null;let e=Pi(n[0]);return e?Ii(e):null}case`cube`:{let e=Object.keys(n)[0]??null;if(!e)return null;let t=n[e],r=Pi(t);return r?Ii(r):null}case`cube-array`:{if(!Array.isArray(n)||n.length===0)return null;let e=n[0],t=Object.keys(e)[0]??null;if(!t)return null;let r=Pi(e[t]);return r?Ii(r):null}default:return null}}function Ii(e){if(ne(e))return ee(e);if(typeof e==`object`&&`width`in e&&`height`in e)return{width:e.width,height:e.height};throw Error(`Unsupported mip-level data`)}function Li(e){return typeof e==`object`&&!!e&&`data`in e&&`width`in e&&`height`in e}function Ri(e){return ArrayBuffer.isView(e)}function zi(e){let{textureFormat:t,format:n}=e;if(t&&n&&t!==n)throw Error(`Conflicting texture formats "${t}" and "${n}" provided for the same mip level`);return t??n}function Bi(e){let t=Ni[e];if(t===void 0)throw Error(`Invalid cube face: ${e}`);return t}function Vi(e,t){return 6*e+Bi(t)}function Hi(e){throw Error(`setTexture1DData not supported in WebGL.`)}function Ui(e){return Array.isArray(e)?e:[e]}function Wi(e,t,n,r){let i=Ui(t),a=e,o=[];for(let e=0;e<i.length;e++){let t=i[e];if(ne(t))o.push({type:`external-image`,image:t,z:a,mipLevel:e});else if(Li(t))o.push({type:`texture-data`,data:t,textureFormat:zi(t),z:a,mipLevel:e});else if(Ri(t)&&n)o.push({type:`texture-data`,data:{data:t,width:Math.max(1,n.width>>e),height:Math.max(1,n.height>>e),...r?{format:r}:{}},textureFormat:r,z:a,mipLevel:e});else throw Error(`Unsupported 2D mip-level payload`)}return o}function Gi(e){let t=[];for(let n=0;n<e.length;n++)t.push(...Wi(n,e[n]));return t}function Ki(e){let t=[];for(let n=0;n<e.length;n++)t.push(...Wi(n,e[n]));return t}function qi(e){let t=[];for(let[n,r]of Object.entries(e)){let e=Bi(n);t.push(...Wi(e,r))}return t}function Ji(e){let t=[];return e.forEach((e,n)=>{for(let[r,i]of Object.entries(e)){let e=Vi(n,r);t.push(...Wi(e,i))}}),t}var Yi=class e{device;id;props;_texture=null;_sampler=null;_view=null;ready;isReady=!1;destroyed=!1;generation=0;updateTimestamp;resolveReady=()=>{};rejectReady=()=>{};get texture(){if(!this._texture)throw Error(`Texture not initialized yet`);return this._texture}get sampler(){if(!this._sampler)throw Error(`Sampler not initialized yet`);return this._sampler}get view(){if(!this._view)throw Error(`View not initialized yet`);return this._view}get[Symbol.toStringTag](){return`DynamicTexture`}toString(){let e=this._texture?.width??this.props.width??`?`,t=this._texture?.height??this.props.height??`?`;return`DynamicTexture:"${this.id}":${e}x${t}px:(${this.isReady?`ready`:`loading...`})`}resolveTextureBinding(e){return this.isReady?this.texture:null}constructor(t,n){this.device=t;let r=f(`dynamic-texture`),i=n;this.props={...e.defaultProps,id:r,...n,data:null},this.id=this.props.id,this.ready=new Promise((e,t)=>{this.resolveReady=e,this.rejectReady=t}),this.updateTimestamp=this.device.incrementTimestamp(),this.initAsync(i)}async initAsync(e){try{let t=await this._loadAllData(e);this._checkNotDestroyed();let n=t.data?Xi({...t,width:e.width,height:e.height,format:e.format}):[],r=`format`in e&&e.format!==void 0,i=`usage`in e&&e.usage!==void 0,a=this.props.width&&this.props.height?{width:this.props.width,height:this.props.height}:Fi(t)||{width:this.props.width||1,height:this.props.height||1};if(!a||a.width<=0||a.height<=0)throw Error(`${this} size could not be determined or was zero`);let o=Zi(this.device,n,a,{format:r?e.format:void 0}),s=o.format??this.props.format,c={...this.props,...a,format:s,mipLevels:1,data:void 0};this.device.isTextureFormatCompressed(s)&&!i&&(c.usage=M.SAMPLE|M.COPY_DST);let l=this.props.mipmaps&&!o.hasExplicitMipChain&&!this.device.isTextureFormatCompressed(s);if(this.device.type===`webgpu`&&l){let e=this.props.dimension===`3d`?M.SAMPLE|M.STORAGE|M.COPY_DST|M.COPY_SRC:M.SAMPLE|M.RENDER|M.COPY_DST|M.COPY_SRC;c.usage|=e}let u=this.device.getMipLevelCount(c.width,c.height),d=o.hasExplicitMipChain?o.mipLevels:this.props.mipLevels===`auto`?u:Math.max(1,Math.min(u,this.props.mipLevels??1)),f={...c,mipLevels:d};this._texture=this.device.createTexture(f),this._sampler=this.texture.sampler,this._view=this.texture.view,this._touchGeneration(),o.subresources.length&&this._setTextureSubresources(o.subresources),this.props.mipmaps&&!o.hasExplicitMipChain&&!l&&w.warn(`${this} skipping auto-generated mipmaps for compressed texture format`)(),l&&this.generateMipmaps(),this.isReady=!0,this.resolveReady(this.texture),w.info(1,`${this} created`)()}catch(e){let t=e instanceof Error?e:Error(String(e));this.rejectReady(t)}}destroy(){this._texture&&(this._texture.destroy(),this._texture=null,this._sampler=null,this._view=null),this.isReady=!1,this.destroyed=!0}generateMipmaps(){this.device.type===`webgl`?(this.texture.generateMipmapsWebGL(),this._touch()):this.device.type===`webgpu`?(this.device.generateMipmapsWebGPU(this.texture),this._touch()):w.warn(`${this} mipmaps not supported on ${this.device.type}`)}setSampler(e={}){this._checkReady();let t=e instanceof se?e:this.device.createSampler(e);this.texture.setSampler(t),this._sampler=t,this._touchGeneration()}async readBuffer(e={}){this.isReady||await this.ready;let t=e.width??this.texture.width,n=e.height??this.texture.height,r=e.depthOrArrayLayers??this.texture.depth,i=this.texture.computeMemoryLayout({width:t,height:n,depthOrArrayLayers:r}),a=this.device.createBuffer({byteLength:i.byteLength,usage:C.COPY_DST|C.MAP_READ});this.texture.readBuffer({...e,width:t,height:n,depthOrArrayLayers:r},a);let o=this.device.createFence();return await o.signaled,o.destroy(),a}async readAsync(e={}){this.isReady||await this.ready;let t=e.width??this.texture.width,n=e.height??this.texture.height,r=e.depthOrArrayLayers??this.texture.depth,i=this.texture.computeMemoryLayout({width:t,height:n,depthOrArrayLayers:r}),a=await this.readBuffer(e),o=await a.readAsync(0,i.byteLength);return a.destroy(),o.buffer instanceof ArrayBuffer?o.buffer:o.slice().buffer}resize(e){if(this._checkReady(),e.width===this.texture.width&&e.height===this.texture.height)return!1;let t=this.texture;return this._texture=t.clone(e),this._sampler=this.texture.sampler,this._view=this.texture.view,t.destroy(),this._touchGeneration(),w.info(`${this} resized`),!0}getCubeFaceIndex(e){let t=Ni[e];if(t===void 0)throw Error(`Invalid cube face: ${e}`);return t}getCubeArrayFaceIndex(e,t){return 6*e+this.getCubeFaceIndex(t)}setTexture1DData(e){if(this._checkReady(),this.texture.props.dimension!==`1d`)throw Error(`${this} is not 1d`);let t=Hi(e);this._setTextureSubresources(t)}setTexture2DData(e,t=0){if(this._checkReady(),this.texture.props.dimension!==`2d`)throw Error(`${this} is not 2d`);let n=Wi(t,e);this._setTextureSubresources(n)}setTexture3DData(e){if(this.texture.props.dimension!==`3d`)throw Error(`${this} is not 3d`);let t=Gi(e);this._setTextureSubresources(t)}setTextureArrayData(e){if(this.texture.props.dimension!==`2d-array`)throw Error(`${this} is not 2d-array`);let t=Ki(e);this._setTextureSubresources(t)}setTextureCubeData(e){if(this.texture.props.dimension!==`cube`)throw Error(`${this} is not cube`);let t=qi(e);this._setTextureSubresources(t)}setTextureCubeArrayData(e){if(this.texture.props.dimension!==`cube-array`)throw Error(`${this} is not cube-array`);let t=Ji(e);this._setTextureSubresources(t)}_setTextureSubresources(e){for(let t of e){let{z:e,mipLevel:n}=t;switch(t.type){case`external-image`:let{image:r,flipY:i}=t;this.texture.copyExternalImage({image:r,z:e,mipLevel:n,flipY:i});break;case`texture-data`:let{data:a,textureFormat:o}=t;if(o&&o!==this.texture.format)throw Error(`${this} mip level ${n} uses format "${o}" but texture format is "${this.texture.format}"`);this.texture.writeData(a.data,{x:0,y:0,z:e,width:a.width,height:a.height,depthOrArrayLayers:1,mipLevel:n});break;default:throw Error(`Unsupported 2D mip-level payload`)}}e.length>0&&this._touch()}async _loadAllData(e){let t=await ta(e.data);return{dimension:e.dimension??`2d`,data:t??null}}_checkNotDestroyed(){this.destroyed&&w.warn(`${this} already destroyed`)}_checkReady(){this.isReady||w.warn(`${this} Cannot perform this operation before ready`)}_touch(){this.updateTimestamp=this.device.incrementTimestamp()}_touchGeneration(){this.generation++,this._touch()}static defaultProps={...M.defaultProps,dimension:`2d`,data:null,mipmaps:!1}};function Xi(e){if(!e.data)return[];let t=e.width&&e.height?{width:e.width,height:e.height}:void 0,n=`format`in e?e.format:void 0;switch(e.dimension){case`1d`:return Hi(e.data);case`2d`:return Wi(0,e.data,t,n);case`3d`:return Gi(e.data);case`2d-array`:return Ki(e.data);case`cube`:return qi(e.data);case`cube-array`:return Ji(e.data);default:throw Error(`Unhandled dimension ${e.dimension}`)}}function Zi(e,t,n,r){if(t.length===0)return{subresources:t,mipLevels:1,format:r.format,hasExplicitMipChain:!1};let i=new Map;for(let e of t){let t=i.get(e.z)??[];t.push(e),i.set(e.z,t)}let a=t.some(e=>e.mipLevel>0),o=r.format,s=1/0,c=[];for(let[t,r]of i){let i=[...r].sort((e,t)=>e.mipLevel-t.mipLevel),a=i[0];if(!a||a.mipLevel!==0)throw Error(`DynamicTexture: slice ${t} is missing mip level 0`);let l=$i(e,a);if(l.width!==n.width||l.height!==n.height)throw Error(`DynamicTexture: slice ${t} base level dimensions ${l.width}x${l.height} do not match expected ${n.width}x${n.height}`);let u=Qi(a);if(u){if(o&&o!==u)throw Error(`DynamicTexture: slice ${t} base level format "${u}" does not match texture format "${o}"`);o=u}let d=o&&e.isTextureFormatCompressed(o)?ea(e,l.width,l.height,o):e.getMipLevelCount(l.width,l.height),f=0;for(let t=0;t<i.length;t++){let n=i[t];if(!n||n.mipLevel!==t||t>=d)break;let r=$i(e,n),a=Math.max(1,l.width>>t),s=Math.max(1,l.height>>t);if(r.width!==a||r.height!==s)break;let u=Qi(n);if(u&&(o||=u,u!==o))break;f++,c.push(n)}s=Math.min(s,f)}let l=Number.isFinite(s)?Math.max(1,s):1;return{subresources:c.filter(e=>e.mipLevel<l),mipLevels:l,format:o,hasExplicitMipChain:a}}function Qi(e){if(e.type===`texture-data`)return e.textureFormat??zi(e.data)}function $i(e,t){switch(t.type){case`external-image`:return e.getExternalImageSize(t.image);case`texture-data`:return{width:t.data.width,height:t.data.height};default:throw Error(`Unsupported texture subresource`)}}function ea(e,t,n,r){let{blockWidth:i=1,blockHeight:a=1}=e.getTextureFormatInfo(r),o=1;for(let e=1;;e++){let r=Math.max(1,t>>e),s=Math.max(1,n>>e);if(r<i||s<a)break;o++}return o}async function ta(e){if(e=await e,Array.isArray(e))return await Promise.all(e.map(ta));if(e&&typeof e==`object`&&e.constructor===Object){let t=e,n=await Promise.all(Object.values(t).map(ta)),r=Object.keys(t),i={};for(let e=0;e<r.length;e++)i[r[e]]=n[e];return i}return e}function na(e,t){if(!e)throw Error(t)}var ra=class{id;matrix=new R;display=!0;position=new I;rotation=new I;scale=new I(1,1,1);userData={};props={};constructor(e={}){let{id:t}=e;this.id=t||f(this.constructor.name),this._setScenegraphNodeProps(e)}getBounds(){return null}destroy(){}delete(){this.destroy()}setProps(e){return this._setScenegraphNodeProps(e),this}toString(){return`{type: ScenegraphNode, id: ${this.id})}`}setPosition(e){return na(e.length===3,`setPosition requires vector argument`),this.position=e,this}setRotation(e){return na(e.length===3||e.length===4,`setRotation requires vector argument`),this.rotation=e,this}setScale(e){return na(e.length===3,`setScale requires vector argument`),this.scale=e,this}setMatrix(e,t=!0){t?this.matrix.copy(e):this.matrix=e}setMatrixComponents(e){let{position:t,rotation:n,scale:r,update:i=!0}=e;return t&&this.setPosition(t),n&&this.setRotation(n),r&&this.setScale(r),i&&this.updateMatrix(),this}updateMatrix(){if(this.matrix.identity(),this.matrix.translate(this.position),this.rotation.length===4){let e=new R().fromQuaternion(this.rotation);this.matrix.multiplyRight(e)}else this.matrix.rotateXYZ(this.rotation);return this.matrix.scale(this.scale),this}update({position:e,rotation:t,scale:n}={}){return e&&this.setPosition(e),t&&this.setRotation(t),n&&this.setScale(n),this.updateMatrix(),this}getCoordinateUniforms(e,t){t||=this.matrix;let n=new R(e).multiplyRight(t),r=n.invert(),i=r.transpose();return{viewMatrix:e,modelMatrix:t,objectMatrix:t,worldMatrix:n,worldInverseMatrix:r,worldInverseTransposeMatrix:i}}_setScenegraphNodeProps(e){e.display!==void 0&&(this.display=e.display),e?.position&&this.setPosition(e.position),e?.rotation&&this.setRotation(e.rotation),e?.scale&&this.setScale(e.scale),this.updateMatrix(),e?.matrix&&this.setMatrix(e.matrix),Object.assign(this.props,e)}};function ia(){return[[1/0,1/0,1/0],[-1/0,-1/0,-1/0]]}function aa(e,t,n){let r=new R(n);for(let n=0;n<8;n++){let i=new I(t[n&1?1:0][0],t[n&2?1:0][1],t[n&4?1:0][2]);r.transformAsPoint(i,i);for(let t=0;t<3;t++)e[0][t]=Math.min(e[0][t],i[t]),e[1][t]=Math.max(e[1][t],i[t])}}function oa(e){return Number.isFinite(e[0][0])}var sa=class e extends ra{children;constructor(e={}){e=Array.isArray(e)?{children:e}:e;let{children:t=[]}=e;w.assert(t.every(e=>e instanceof ra),`every child must an instance of ScenegraphNode`),super(e),this.children=t}getBounds(){let e=ia();return this.traverse((t,{worldMatrix:n})=>{let r=t.getBounds();if(!r)return;let i=new R(n).multiplyRight(t.matrix);aa(e,r,i)}),oa(e)?e:null}destroy(){this.children.forEach(e=>e.destroy()),this.removeAll(),super.destroy()}add(...e){for(let t of e)Array.isArray(t)?this.add(...t):this.children.push(t);return this}remove(e){let t=this.children,n=t.indexOf(e);return n>-1&&t.splice(n,1),this}removeAll(){return this.children=[],this}traverse(t,{worldMatrix:n=new R}={}){if(!this.display)return;let r=new R(n).multiplyRight(this.matrix);for(let n of this.children)n.display&&(n instanceof e?n.traverse(t,{worldMatrix:r}):t(n,{worldMatrix:r}))}traverseDepthSorted(e,{viewMatrix:t,worldMatrix:n=new R,order:r=`back-to-front`}){let i=new R(t),a=[];this.traverse((e,t)=>{let n=e.getBounds(),r=n?new I(n[0]).add(n[1]).divide([2,2,2]):new I,o=new R(t.worldMatrix).multiplyRight(e.matrix);o.transformAsPoint(r,r),i.transformAsPoint(r,r),a.push({node:e,context:{worldMatrix:o,bounds:n,depth:-r[2]},index:a.length})},{worldMatrix:new R(n)});let o=r===`back-to-front`?-1:1;a.sort((e,t)=>o*(e.context.depth-t.context.depth)||e.index-t.index);for(let{node:t,context:n}of a)e(t,n)}preorderTraversal(t,{worldMatrix:n=new R}={}){let r=new R(n).multiplyRight(this.matrix);t(this,{worldMatrix:r});for(let n of this.children)n instanceof e?n.preorderTraversal(t,{worldMatrix:r}):t(n,{worldMatrix:r})}},ca=class extends ra{model;instanceMatrices;bounds=null;managedResources;constructor(e){super(e),this.model=e.model,this.managedResources=e.managedResources||[],this.instanceMatrices=e.instanceMatrices||null,this.bounds=e.bounds?this.instanceMatrices?la(e.bounds,this.instanceMatrices):e.bounds:null,this.setProps(e)}destroy(){this.model&&=(this.model.destroy(),null),this.managedResources.forEach(e=>e.destroy()),this.managedResources=[]}getBounds(){return this.bounds}draw(e){return this.model.draw(e)}};function la(e,t){let n=ia();for(let r of t)aa(n,e,r);return oa(n)?n:null}var ua={WEBGL_depth_texture:{UNSIGNED_INT_24_8_WEBGL:34042},OES_element_index_uint:{},OES_texture_float:{},OES_texture_half_float:{HALF_FLOAT_OES:5131},EXT_color_buffer_float:{},OES_standard_derivatives:{FRAGMENT_SHADER_DERIVATIVE_HINT_OES:35723},EXT_frag_depth:{},EXT_blend_minmax:{MIN_EXT:32775,MAX_EXT:32776},EXT_shader_texture_lod:{}},da=e=>({drawBuffersWEBGL(t){return e.drawBuffers(t)},COLOR_ATTACHMENT0_WEBGL:36064,COLOR_ATTACHMENT1_WEBGL:36065,COLOR_ATTACHMENT2_WEBGL:36066,COLOR_ATTACHMENT3_WEBGL:36067}),fa=e=>({VERTEX_ARRAY_BINDING_OES:34229,createVertexArrayOES(){return e.createVertexArray()},deleteVertexArrayOES(t){return e.deleteVertexArray(t)},isVertexArrayOES(t){return e.isVertexArray(t)},bindVertexArrayOES(t){return e.bindVertexArray(t)}}),pa=e=>({VERTEX_ATTRIB_ARRAY_DIVISOR_ANGLE:35070,drawArraysInstancedANGLE(...t){return e.drawArraysInstanced(...t)},drawElementsInstancedANGLE(...t){return e.drawElementsInstanced(...t)},vertexAttribDivisorANGLE(...t){return e.vertexAttribDivisor(...t)}});function ma(e=!0){let t=HTMLCanvasElement.prototype;if(!e&&t.originalGetContext){t.getContext=t.originalGetContext,t.originalGetContext=void 0;return}t.originalGetContext=t.getContext,t.getContext=function(e,t){if(e===`webgl`||e===`experimental-webgl`){let e=this.originalGetContext(`webgl2`,t);return e instanceof HTMLElement&&ha(e),e}return this.originalGetContext(e,t)}}function ha(e){e.getExtension(`EXT_color_buffer_float`);let t={...ua,WEBGL_disjoint_timer_query:e.getExtension(`EXT_disjoint_timer_query_webgl2`),WEBGL_draw_buffers:da(e),OES_vertex_array_object:fa(e),ANGLE_instanced_arrays:pa(e)},n=e.getExtension.bind(e);e.getExtension=function(e){return n(e)||(e in t?t[e]:null)};let r=e.getSupportedExtensions;e.getSupportedExtensions=function(){return(r.apply(e)||[])?.concat(Object.keys(t))}}var ga=1,_a=class extends be{type=`webgl`;enforceWebGL2(e){ma(e)}isSupported(){return typeof WebGL2RenderingContext<`u`}isDeviceHandle(e){return typeof WebGL2RenderingContext<`u`&&e instanceof WebGL2RenderingContext?!0:(typeof WebGLRenderingContext<`u`&&e instanceof WebGLRenderingContext&&w.warn(`WebGL1 is not supported`,e)(),!1)}async attach(e,t={}){let{WebGLDevice:n}=await o(async()=>{let{WebGLDevice:e}=await import(`./webgl-device-McYZ27Ud.js`).then(e=>e.n);return{WebGLDevice:e}},__vite__mapDeps([0,1,2,3]),import.meta.url);if(e instanceof n)return e;let r=n.getDeviceFromContext(e);if(r)return r;if(!va(e))throw Error(`Invalid WebGL2RenderingContext`);t=ba(t),await xa(t);let i=t.createCanvasContext===!0?{}:t.createCanvasContext;return new n({...t,_handle:e,createCanvasContext:{canvas:e.canvas,autoResize:!1,...i}})}async create(e={}){let{WebGLDevice:t}=await o(async()=>{let{WebGLDevice:e}=await import(`./webgl-device-McYZ27Ud.js`).then(e=>e.n);return{WebGLDevice:e}},__vite__mapDeps([0,1,2,3]),import.meta.url);e=ba(e),await xa(e);try{let n=new t(e);w.groupCollapsed(ga,`WebGLDevice ${n.id} created`)();let r=`\
${n._reused?`Reusing`:`Created`} device with WebGL2 ${n.props.debug?`debug `:``}context: \
${n.info.vendor}, ${n.info.renderer} for canvas: ${n.canvasContext.id}`;return w.probe(ga,r)(),w.table(ga,n.info)(),n}finally{w.groupEnd(ga)(),w.info(ga,`%cWebGL call tracing: luma.log.set('debug-webgl') `,`color: white; background: blue; padding: 2px 6px; border-radius: 3px;`)()}}};function va(e){return typeof WebGL2RenderingContext<`u`&&e instanceof WebGL2RenderingContext||!!(e&&typeof e.createVertexArray==`function`)}var ya=new _a;function ba(e){return{...e,debug:e.debug??O.defaultProps.debug,debugWebGL:e.debugWebGL??O.defaultProps.debugWebGL,debugSpectorJS:e.debugSpectorJS??!!w.get(`debug-spectorjs`)}}async function xa(e){let t=[];(e.debugWebGL||e.debug)&&t.push(A()),e.debugSpectorJS&&t.push(te(e));let n=await Promise.allSettled(t);for(let e of n)e.status===`rejected`&&w.error(`Failed to initialize debug libraries ${e.reason}`)()}function Sa(e,t){if(!e)throw Error(t||`loader assertion failed.`)}var Ca={self:typeof self<`u`&&self,window:typeof window<`u`&&window,global:typeof global<`u`&&global,document:typeof document<`u`&&document};Ca.self||Ca.window||Ca.global,Ca.window||Ca.self||Ca.global,Ca.global||Ca.self||Ca.window,Ca.document;var wa=!!(typeof process!=`object`||String(process)!==`[object process]`||process.browser),Ta=typeof process<`u`&&process.version&&/v([0-9]*)/.exec(process.version);Ta&&parseFloat(Ta[1]);var Ea=`v4.5.4`;function Da(){let e=new D({id:`loaders.gl`});return globalThis.loaders||={},globalThis.loaders.log=e,globalThis.loaders.version=Ea,globalThis.probe||={},globalThis.probe.loaders=e,e}var Oa=Da(),ka=e=>typeof e==`boolean`,Aa=e=>typeof e==`function`,ja=e=>typeof e==`object`&&!!e,Ma=e=>ja(e)&&e.constructor==={}.constructor,Na=e=>typeof SharedArrayBuffer<`u`&&e instanceof SharedArrayBuffer,Pa=e=>ja(e)&&typeof e.byteLength==`number`&&typeof e.slice==`function`,Fa=e=>!!e&&Aa(e[Symbol.iterator]),Ia=e=>!!e&&Aa(e[Symbol.asyncIterator]),La=e=>typeof Response<`u`&&e instanceof Response||ja(e)&&Aa(e.arrayBuffer)&&Aa(e.text)&&Aa(e.json),Ra=e=>typeof Blob<`u`&&e instanceof Blob,za=e=>typeof ReadableStream<`u`&&e instanceof ReadableStream||ja(e)&&Aa(e.tee)&&Aa(e.cancel)&&Aa(e.getReader),Ba=e=>ja(e)&&Aa(e.read)&&Aa(e.pipe)&&ka(e.readable),Va=e=>za(e)||Ba(e);function Ha(e,t){return Ua(e||{},t)}function Ua(e,t,n=0){if(n>3)return t;let r={...e};for(let[e,i]of Object.entries(t))i&&typeof i==`object`&&!Array.isArray(i)?r[e]=Ua(r[e]||{},t[e],n+1):r[e]=t[e];return r}var Wa=`latest`;function Ga(){return globalThis._loadersgl_?.version||(globalThis._loadersgl_=globalThis._loadersgl_||{},globalThis._loadersgl_.version=`4.5.4`),globalThis._loadersgl_.version}var Ka=Ga();function qa(e,t){if(!e)throw Error(t||`loaders.gl assertion failed.`)}var Ja={self:typeof self<`u`&&self,window:typeof window<`u`&&window,global:typeof global<`u`&&global,document:typeof document<`u`&&document};Ja.self||Ja.window||Ja.global,Ja.window||Ja.self||Ja.global,Ja.global||Ja.self||Ja.window,Ja.document;var Ya=typeof process!=`object`||String(process)!==`[object process]`||process.browser,Xa=typeof window<`u`&&window.orientation!==void 0,Za=typeof process<`u`&&process.version&&/v([0-9]*)/.exec(process.version);Za&&parseFloat(Za[1]);var Qa=class{name;workerThread;isRunning=!0;result;_resolve=()=>{};_reject=()=>{};constructor(e,t){this.name=e,this.workerThread=t,this.result=new Promise((e,t)=>{this._resolve=e,this._reject=t})}postMessage(e,t){this.workerThread.postMessage({source:`loaders.gl`,type:e,payload:t})}done(e){qa(this.isRunning),this.isRunning=!1,this._resolve(e)}error(e){qa(this.isRunning),this.isRunning=!1,this._reject(e)}},$a=class{terminate(){}},eo=new Map;function to(e){qa(e.source&&!e.url||!e.source&&e.url);let t=eo.get(e.source||e.url);return t||(e.url&&(t=no(e.url),eo.set(e.url,t)),e.source&&(t=ro(e.source),eo.set(e.source,t))),qa(t),t}function no(e){return e.startsWith(`http`)?ro(io(e)):e}function ro(e){let t=new Blob([e],{type:`application/javascript`});return URL.createObjectURL(t)}function io(e){return`\
try {
  importScripts('${e}');
} catch (error) {
  console.error(error);
  throw error;
}`}function ao(e,t=!0,n){let r=n||new Set;if(e){if(oo(e))r.add(e);else if(oo(e.buffer))r.add(e.buffer);else if(!ArrayBuffer.isView(e)&&t&&typeof e==`object`)for(let n in e)ao(e[n],t,r)}return n===void 0?Array.from(r):[]}function oo(e){return e?e instanceof ArrayBuffer||typeof MessagePort<`u`&&e instanceof MessagePort||typeof ImageBitmap<`u`&&e instanceof ImageBitmap||typeof OffscreenCanvas<`u`&&e instanceof OffscreenCanvas:!1}var so=()=>{},co=class{name;source;url;terminated=!1;worker;onMessage;onError;_loadableURL=``;static isSupported(){return typeof Worker<`u`&&Ya||$a!==void 0&&!Ya}constructor(e){let{name:t,source:n,url:r}=e;qa(n||r),this.name=t,this.source=n,this.url=r,this.onMessage=so,this.onError=e=>console.log(e),this.worker=Ya?this._createBrowserWorker():this._createNodeWorker()}destroy(){this.onMessage=so,this.onError=so,this.worker.terminate(),this.terminated=!0}get isRunning(){return!!this.onMessage}postMessage(e,t){t||=ao(e),this.worker.postMessage(e,t)}_getErrorFromErrorEvent(e){let t=`Failed to load `;return t+=`worker ${this.name} from ${this.url}. `,e.message&&(t+=`${e.message} in `),e.lineno&&(t+=`:${e.lineno}:${e.colno}`),Error(t)}_createBrowserWorker(){this._loadableURL=to({source:this.source,url:this.url});let e=new Worker(this._loadableURL,{name:this.name});return e.onmessage=e=>{e.data?this.onMessage(e.data):this.onError(Error(`No data received`))},e.onerror=e=>{this.onError(this._getErrorFromErrorEvent(e)),this.terminated=!0},e.onmessageerror=e=>console.error(e),e}_createNodeWorker(){let e;if(this.url)e=new $a(this.url.includes(`:/`)||this.url.startsWith(`/`)?this.url:`./${this.url}`,{eval:!1,type:this.url.endsWith(`.ts`)||this.url.endsWith(`.mjs`)?`module`:`commonjs`});else if(this.source)e=new $a(this.source,{eval:!0});else throw Error(`no worker`);return e.on(`message`,e=>{this.onMessage(e)}),e.on(`error`,e=>{this.onError(e)}),e.on(`exit`,e=>{}),e}},lo=class{name=`unnamed`;source;url;maxConcurrency=1;maxMobileConcurrency=1;onDebug=()=>{};reuseWorkers=!0;props={};jobQueue=[];idleQueue=[];count=0;isDestroyed=!1;static isSupported(){return co.isSupported()}constructor(e){this.source=e.source,this.url=e.url,this.setProps(e)}destroy(){this.idleQueue.forEach(e=>e.destroy()),this.isDestroyed=!0}setProps(e){this.props={...this.props,...e},e.name!==void 0&&(this.name=e.name),e.maxConcurrency!==void 0&&(this.maxConcurrency=e.maxConcurrency),e.maxMobileConcurrency!==void 0&&(this.maxMobileConcurrency=e.maxMobileConcurrency),e.reuseWorkers!==void 0&&(this.reuseWorkers=e.reuseWorkers),e.onDebug!==void 0&&(this.onDebug=e.onDebug)}async startJob(e,t=(e,t,n)=>e.done(n),n=(e,t)=>e.error(t)){let r=new Promise(r=>(this.jobQueue.push({name:e,onMessage:t,onError:n,onStart:r}),this));return this._startQueuedJob(),await r}async _startQueuedJob(){if(!this.jobQueue.length)return;let e=this._getAvailableWorker();if(!e)return;let t=this.jobQueue.shift();if(t){this.onDebug({message:`Starting job`,name:t.name,workerThread:e,backlog:this.jobQueue.length});let n=new Qa(t.name,e);e.onMessage=e=>t.onMessage(n,e.type,e.payload),e.onError=e=>t.onError(n,e),t.onStart(n);try{await n.result}catch(e){console.error(`Worker exception: ${e}`)}finally{this.returnWorkerToQueue(e)}}}returnWorkerToQueue(e){!Ya||this.isDestroyed||!this.reuseWorkers||this.count>this._getMaxConcurrency()?(e.destroy(),this.count--):this.idleQueue.push(e),this.isDestroyed||this._startQueuedJob()}_getAvailableWorker(){return this.idleQueue.length>0?this.idleQueue.shift()||null:this.count<this._getMaxConcurrency()?(this.count++,new co({name:`${this.name.toLowerCase()} (#${this.count} of ${this.maxConcurrency})`,source:this.source,url:this.url})):null}_getMaxConcurrency(){return Xa?this.maxMobileConcurrency:this.maxConcurrency}},uo={maxConcurrency:3,maxMobileConcurrency:1,reuseWorkers:!0,onDebug:()=>{}},fo=class e{props;workerPools=new Map;static _workerFarm;static isSupported(){return co.isSupported()}static getWorkerFarm(t={}){return e._workerFarm=e._workerFarm||new e({}),e._workerFarm.setProps(t),e._workerFarm}constructor(e){this.props={...uo},this.setProps(e),this.workerPools=new Map}destroy(){for(let e of this.workerPools.values())e.destroy();this.workerPools=new Map}setProps(e){this.props={...this.props,...e};for(let e of this.workerPools.values())e.setProps(this._getWorkerPoolProps())}getWorkerPool(e){let{name:t,source:n,url:r}=e,i=this.workerPools.get(t);return i||(i=new lo({name:t,source:n,url:r}),i.setProps(this._getWorkerPoolProps()),this.workerPools.set(t,i)),i}_getWorkerPoolProps(){return{maxConcurrency:this.props.maxConcurrency,maxMobileConcurrency:this.props.maxMobileConcurrency,reuseWorkers:this.props.reuseWorkers,onDebug:this.props.onDebug}}};function po(e,t={}){let n=t[e.id]||{},r=Ya?e.workerFile||`${e.id}-worker.js`:`${e.id}-worker-node.js`,i=n.workerUrl;if(!i&&e.id===`compression`&&(i=t.workerUrl),(t._workerType||t?.core?._workerType)===`test`&&(i=Ya?`modules/${e.module}/dist/${r}`:`modules/${e.module}/src/workers/${e.id}-worker-node.ts`),!i){let t=e.version;t===`latest`&&(t=Wa);let n=t?`@${t}`:``;i=`https://unpkg.com/@loaders.gl/${e.module}${n}/dist/${r}`}return qa(i),i}function mo(e,t=Ka){qa(e,`no worker provided`);let n=e.version;return!(!t||!n)}function ho(e,t){if(!fo.isSupported())return!1;let n=t?._nodeWorkers??t?.core?._nodeWorkers;if(!Ya&&!n)return!1;let r=t?.worker??t?.core?.worker;return!!(e.worker&&r)}async function go(e,t,n,r,i){let a=e.id,o=po(e,n),s=fo.getWorkerFarm(n?.core).getWorkerPool({name:a,url:o});n=JSON.parse(JSON.stringify(n||{})),n._workerLoaderId=e.id,r=JSON.parse(JSON.stringify(r||{}));let c=await s.startJob(`process-on-worker`,_o.bind(null,i));return c.postMessage(`process`,{input:t,options:n,context:r}),await(await c.result).result}async function _o(e,t,n,r){switch(n){case`done`:t.done(r);break;case`error`:t.error(Error(r.error));break;case`process`:let{id:i,input:a,options:o}=r;try{let n=await e(a,o);t.postMessage(`done`,{id:i,result:n})}catch(e){let n=e instanceof Error?e.message:`unknown error`;t.postMessage(`error`,{id:i,error:n})}break;default:console.warn(`parse-with-worker unknown message ${n}`)}}function vo(e,t,n){if(n||=e.byteLength,e.byteLength<n||t.byteLength<n)return!1;let r=new Uint8Array(e),i=new Uint8Array(t);for(let e=0;e<r.length;++e)if(r[e]!==i[e])return!1;return!0}function yo(...e){return bo(e)}function bo(e){let t=e.map(e=>e instanceof ArrayBuffer?new Uint8Array(e):e),n=t.reduce((e,t)=>e+t.byteLength,0),r=new Uint8Array(n),i=0;for(let e of t)r.set(e,i),i+=e.byteLength;return r.buffer}async function xo(e){let t=[];for await(let n of e)t.push(So(n));return yo(...t)}function So(e){if(e instanceof ArrayBuffer)return e;if(ArrayBuffer.isView(e)){let{buffer:t,byteOffset:n,byteLength:r}=e;return Co(t,n,r)}return Co(e)}function Co(e,t=0,n=e.byteLength-t){let r=new Uint8Array(e,t,n),i=new Uint8Array(r.length);return i.set(r),i.buffer}var wo=``,To={};function Eo(e){for(let t in To)if(e.startsWith(t)){let n=To[t];e=e.replace(t,n)}return!e.startsWith(`http://`)&&!e.startsWith(`https://`)&&(e=`${wo}${e}`),e}function Do(e){return e}function Oo(e){return e&&typeof e==`object`&&e.isBuffer}function ko(e){if(Oo(e))return Do(e);if(e instanceof ArrayBuffer)return e;if(Na(e))return jo(e);if(ArrayBuffer.isView(e)){let t=e.buffer;return e.byteOffset===0&&e.byteLength===e.buffer.byteLength?t:t.slice(e.byteOffset,e.byteOffset+e.byteLength)}if(typeof e==`string`){let t=e;return new TextEncoder().encode(t).buffer}if(e&&typeof e==`object`&&e._toArrayBuffer)return e._toArrayBuffer();throw Error(`toArrayBuffer`)}function Ao(e){if(e instanceof ArrayBuffer)return e;if(Na(e))return jo(e);let{buffer:t,byteOffset:n,byteLength:r}=e;return t instanceof ArrayBuffer&&n===0&&r===t.byteLength?t:jo(t,n,r)}function jo(e,t=0,n=e.byteLength-t){let r=new Uint8Array(e,t,n),i=new Uint8Array(r.length);return i.set(r),i.buffer}function Mo(e){return ArrayBuffer.isView(e)?e:new Uint8Array(e)}function No(e){let t=e?e.lastIndexOf(`/`):-1;return t>=0?e.substr(t+1):e}function Po(e){let t=e?e.lastIndexOf(`/`):-1;return t>=0?e.substr(0,t):``}var Fo=class extends Error{constructor(e,t){super(e),this.reason=t.reason,this.url=t.url,this.response=t.response}reason;url;response},Io=/^data:([-\w.]+\/[-\w.+]+)(;|,)/,Lo=/^([-\w.]+\/[-\w.+]+)/;function Ro(e,t){return e.toLowerCase()===t.toLowerCase()}function zo(e){let t=Lo.exec(e);return t?t[1]:e}function Bo(e){let t=Io.exec(e);return t?t[1]:``}var Vo=/\?.*/;function Ho(e){let t=e.match(Vo);return t&&t[0]}function Uo(e){return e.replace(Vo,``)}function Wo(e){if(e.length<50)return e;let t=e.slice(e.length-15);return`${e.substr(0,32)}...${t}`}function Go(e){return La(e)?e.url:Ra(e)?(`name`in e?e.name:``)||``:typeof e==`string`?e:``}function Ko(e){if(La(e)){let t=e.headers.get(`content-type`)||``,n=Uo(e.url);return zo(t)||Bo(n)}return Ra(e)?e.type||``:typeof e==`string`?Bo(e):``}function qo(e){return La(e)?e.headers[`content-length`]||-1:Ra(e)?e.size:typeof e==`string`?e.length:e instanceof ArrayBuffer||ArrayBuffer.isView(e)?e.byteLength:-1}async function Jo(e){if(La(e))return e;let t={},n=qo(e);n>=0&&(t[`content-length`]=String(n));let r=Go(e),i=Ko(e);i&&(t[`content-type`]=i);let a=await Zo(e);a&&(t[`x-first-bytes`]=a),typeof e==`string`&&(e=new TextEncoder().encode(e));let o=new Response(e,{headers:t});return Object.defineProperty(o,"url",{value:r}),o}async function Yo(e){if(!e.ok)throw await Xo(e)}async function Xo(e){let t=Wo(e.url),n=`Failed to fetch resource (${e.status}) ${e.statusText}: ${t}`;n=n.length>100?`${n.slice(0,100)}...`:n;let r={reason:e.statusText,url:e.url,response:e};try{let t=e.headers.get(`Content-Type`);r.reason=!e.bodyUsed&&t?.includes(`application/json`)?await e.json():await e.text()}catch{}return new Fo(n,r)}async function Zo(e){if(typeof e==`string`)return`data:,${e.slice(0,5)}`;if(e instanceof Blob){let t=e.slice(0,5);return await new Promise(e=>{let n=new FileReader;n.onload=t=>e(t?.target?.result),n.readAsDataURL(t)})}return e instanceof ArrayBuffer?`data:base64,${Qo(e.slice(0,5))}`:null}function Qo(e){let t=``,n=new Uint8Array(e);for(let e=0;e<n.byteLength;e++)t+=String.fromCharCode(n[e]);return btoa(t)}function $o(e){return!es(e)&&!ts(e)}function es(e){return e.startsWith(`http:`)||e.startsWith(`https:`)}function ts(e){return e.startsWith(`data:`)}async function ns(e,t){if(typeof e==`string`){let n=Eo(e);return $o(n)&&globalThis.loaders?.fetchNode?globalThis.loaders?.fetchNode(n,t):await fetch(n,t)}return await Jo(e)}var rs=new D({id:`loaders.gl`}),is=class{log(){return()=>{}}info(){return()=>{}}warn(){return()=>{}}error(){return()=>{}}},as={core:{baseUrl:void 0,fetch:null,mimeType:void 0,fallbackMimeType:void 0,ignoreRegisteredLoaders:void 0,nothrow:!1,log:new class{console;constructor(){this.console=console}log(...e){return this.console.log.bind(this.console,...e)}info(...e){return this.console.info.bind(this.console,...e)}warn(...e){return this.console.warn.bind(this.console,...e)}error(...e){return this.console.error.bind(this.console,...e)}},useLocalLibraries:!1,CDN:`https://unpkg.com/@loaders.gl`,worker:!0,maxConcurrency:3,maxMobileConcurrency:1,reuseWorkers:wa,_nodeWorkers:!1,_workerType:``,limit:0,_limitMB:0,batchSize:`auto`,batchDebounceMs:0,metadata:!1,transforms:[]}},os={baseUri:`core.baseUrl`,fetch:`core.fetch`,mimeType:`core.mimeType`,fallbackMimeType:`core.fallbackMimeType`,ignoreRegisteredLoaders:`core.ignoreRegisteredLoaders`,nothrow:`core.nothrow`,log:`core.log`,useLocalLibraries:`core.useLocalLibraries`,CDN:`core.CDN`,worker:`core.worker`,maxConcurrency:`core.maxConcurrency`,maxMobileConcurrency:`core.maxMobileConcurrency`,reuseWorkers:`core.reuseWorkers`,_nodeWorkers:`core.nodeWorkers`,_workerType:`core._workerType`,_worker:`core._workerType`,limit:`core.limit`,_limitMB:`core._limitMB`,batchSize:`core.batchSize`,batchDebounceMs:`core.batchDebounceMs`,metadata:`core.metadata`,transforms:`core.transforms`,throws:`nothrow`,dataType:`(no longer used)`,uri:`core.baseUrl`,method:`core.fetch.method`,headers:`core.fetch.headers`,body:`core.fetch.body`,mode:`core.fetch.mode`,credentials:`core.fetch.credentials`,cache:`core.fetch.cache`,redirect:`core.fetch.redirect`,referrer:`core.fetch.referrer`,referrerPolicy:`core.fetch.referrerPolicy`,integrity:`core.fetch.integrity`,keepalive:`core.fetch.keepalive`,signal:`core.fetch.signal`},ss=[`baseUrl`,`fetch`,`mimeType`,`fallbackMimeType`,`ignoreRegisteredLoaders`,`nothrow`,`log`,`useLocalLibraries`,`CDN`,`worker`,`maxConcurrency`,`maxMobileConcurrency`,`reuseWorkers`,`_nodeWorkers`,`_workerType`,`limit`,`_limitMB`,`batchSize`,`batchDebounceMs`,`metadata`,`transforms`];function cs(){globalThis.loaders=globalThis.loaders||{};let{loaders:e}=globalThis;return e._state||={},e._state}function ls(){let e=cs();return e.globalOptions=e.globalOptions||{...as,core:{...as.core}},ds(e.globalOptions)}function us(e,t,n,r){return n||=[],n=Array.isArray(n)?n:[n],fs(e,n),ds(hs(t,e,r))}function ds(e){let t=vs(e);ys(t);for(let e of ss)t.core&&t.core[e]!==void 0&&delete t[e];return t.core&&t.core._workerType!==void 0&&delete t._worker,t}function fs(e,t){ps(e,null,as,os,t);for(let n of t){let r=e&&e[n.id]||{},i=n.options&&n.options[n.id]||{},a=n.deprecatedOptions&&n.deprecatedOptions[n.id]||{};ps(r,n.id,i,a,t)}}function ps(e,t,n,r,i){let a=t||`Top level`,o=t?`${t}.`:``;for(let s in e){let c=!t&&ja(e[s]),l=s===`baseUri`&&!t,u=s===`workerUrl`&&t;if(!(s in n)&&!l&&!u){if(s in r)rs.level>0&&rs.warn(`${a} loader option \'${o}${s}\' no longer supported, use \'${r[s]}\'`)();else if(!c&&rs.level>0){let e=ms(s,i);rs.warn(`${a} loader option \'${o}${s}\' not recognized. ${e}`)()}}}}function ms(e,t){let n=e.toLowerCase(),r=``;for(let i of t)for(let t in i.options){if(e===t)return`Did you mean \'${i.id}.${t}\'?`;let a=t.toLowerCase();(n.startsWith(a)||a.startsWith(n))&&(r||=`Did you mean \'${i.id}.${t}\'?`)}return r}function hs(e,t,n){let r=e.options||{},i={...r};return r.core&&(i.core={...r.core}),ys(i),i.core?.log===null&&(i.core={...i.core,log:new is}),gs(i,ds(ls())),gs(i,ds(t)),_s(i,n),bs(i),i}function gs(e,t){for(let n in t)if(n in t){let r=t[n];Ma(r)&&Ma(e[n])?e[n]={...e[n],...t[n]}:e[n]=t[n]}}function _s(e,t){t&&e.core?.baseUrl===void 0&&(e.core||={},e.core.baseUrl=Po(Uo(t)))}function vs(e){let t={...e};return e.core&&(t.core={...e.core}),t}function ys(e){e.baseUri!==void 0&&(e.core||={},e.core.baseUrl===void 0&&(e.core.baseUrl=e.baseUri));for(let t of ss)if(e[t]!==void 0){let n=e.core=e.core||{};n[t]===void 0&&(n[t]=e[t])}let t=e._worker;t!==void 0&&(e.core||={},e.core._workerType===void 0&&(e.core._workerType=t))}function bs(e){let t=e.core;if(t)for(let n of ss)t[n]!==void 0&&(e[n]=t[n])}function xs(e){return e?(Array.isArray(e)&&(e=e[0]),Array.isArray(e?.extensions)):!1}function Ss(e){Sa(e,`null loader`),Sa(xs(e),`invalid loader`);let t;return Array.isArray(e)&&(t=e[1],e=e[0],e={...e,options:{...e.options,...t}}),(e?.parseTextSync||e?.parseText)&&(e.text=!0),e.text||(e.binary=!0),e}var Cs=()=>{let e=cs();return e.loaderRegistry=e.loaderRegistry||[],e.loaderRegistry};function ws(e){let t=Cs();e=Array.isArray(e)?e:[e];for(let n of e){let e=Ss(n);t.find(t=>e===t)||t.unshift(e)}}function Ts(){return Cs()}var Es=/\.([^.]+)$/;async function Ds(e,t=[],n,r){if(!js(e))return null;let i=ds(n||{});if(i.core||={},e instanceof Response&&Os(e)){let n=ks(await e.clone().text(),t,{...i,core:{...i.core,nothrow:!0}},r);if(n)return n}let a=ks(e,t,{...i,core:{...i.core,nothrow:!0}},r);if(a)return a;if(Ra(e)&&(e=await e.slice(0,10).arrayBuffer(),a=ks(e,t,i,r)),!a&&e instanceof Response&&Os(e)&&(a=ks(await e.clone().text(),t,i,r)),!a&&!i.core.nothrow)throw Error(Ms(e));return a}function Os(e){let t=Ko(e);return!!(t&&(t.startsWith(`text/`)||t===`application/json`||t.endsWith(`+json`)))}function ks(e,t=[],n,r){if(!js(e))return null;let i=ds(n||{});if(i.core||={},t&&!Array.isArray(t))return Ss(t);let a=[];t&&(a=a.concat(t)),i.core.ignoreRegisteredLoaders||a.push(...Ts()),Ns(a);let o=As(e,a,i,r);if(!o&&!i.core.nothrow)throw Error(Ms(e));return o}function As(e,t,n,r){let i=Go(e),a=Ko(e),o=Uo(i)||r?.url,s=null,c=``;return n?.core?.mimeType&&(s=Is(t,n?.core?.mimeType),c=`match forced by supplied MIME type ${n?.core?.mimeType}`),s||=Ps(t,o),c||=s?`matched url ${o}`:``,s||=Is(t,a),c||=s?`matched MIME type ${a}`:``,s||=Ls(t,e),c||=s?`matched initial data ${Vs(e)}`:``,n?.core?.fallbackMimeType&&(s||=Is(t,n?.core?.fallbackMimeType),c||=s?`matched fallback MIME type ${a}`:``),c&&Oa.log(1,`selectLoader selected ${s?.name}: ${c}.`),s}function js(e){return!(e instanceof Response&&e.status===204)}function Ms(e){let t=Go(e),n=Ko(e),r=`No valid loader found (`;r+=t?`${No(t)}, `:`no url provided, `,r+=`MIME type: ${n?`"${n}"`:`not provided`}, `;let i=e?Vs(e):``;return r+=i?` first bytes: "${i}"`:`first bytes: not available`,r+=`)`,r}function Ns(e){for(let t of e)Ss(t)}function Ps(e,t){let n=t&&Es.exec(t),r=n&&n[1];return r?Fs(e,r):null}function Fs(e,t){t=t.toLowerCase();for(let n of e)for(let e of n.extensions)if(e.toLowerCase()===t)return n;return null}function Is(e,t){for(let n of e)if(n.mimeTypes?.some(e=>Ro(t,e))||Ro(t,`application/x.${n.id}`))return n;return null}function Ls(e,t){if(!t)return null;for(let n of e)if(typeof t==`string`){if(Rs(t,n))return n}else if(ArrayBuffer.isView(t)){if(zs(t.buffer,t.byteOffset,n))return n}else if(t instanceof ArrayBuffer&&zs(t,0,n))return n;return null}function Rs(e,t){return t.testText?t.testText(e):(Array.isArray(t.tests)?t.tests:[t.tests]).some(t=>e.startsWith(t))}function zs(e,t,n){return(Array.isArray(n.tests)?n.tests:[n.tests]).some(r=>Bs(e,t,n,r))}function Bs(e,t,n,r){if(Pa(r))return vo(r,e,r.byteLength);switch(typeof r){case`function`:return r(Ao(e));case`string`:return r===Hs(e,t,r.length);default:return!1}}function Vs(e,t=5){return typeof e==`string`?e.slice(0,t):ArrayBuffer.isView(e)?Hs(e.buffer,e.byteOffset,t):e instanceof ArrayBuffer?Hs(e,0,t):``}function Hs(e,t,n){if(e.byteLength<t+n)return``;let r=new DataView(e),i=``;for(let e=0;e<n;e++)i+=String.fromCharCode(r.getUint8(t+e));return i}var Us=256*1024;function*Ws(e,t){let n=t?.chunkSize||Us,r=0,i=new TextEncoder;for(;r<e.length;){let t=Math.min(e.length-r,n),a=e.slice(r,r+t);r+=t,yield Ao(i.encode(a))}}var Gs=256*1024;function*Ks(e,t={}){let{chunkSize:n=Gs}=t,r=0;for(;r<e.byteLength;){let t=Math.min(e.byteLength-r,n),i=new ArrayBuffer(t),a=new Uint8Array(e,r,t);new Uint8Array(i).set(a),r+=t,yield i}}var qs=1024*1024;async function*Js(e,t){let n=t?.chunkSize||qs,r=0;for(;r<e.size;){let t=r+n,i=await e.slice(r,t).arrayBuffer();r=t,yield i}}function Ys(e,t){return wa?Xs(e,t):Zs(e,t)}async function*Xs(e,t){let n=e.getReader(),r;try{for(;;){let e=r||n.read();t?._streamReadAhead&&(r=n.read());let{done:i,value:a}=await e;if(i)return;yield ko(a)}}catch{n.releaseLock()}}async function*Zs(e,t){for await(let t of e)yield ko(t)}function Qs(e,t){if(typeof e==`string`)return Ws(e,t);if(e instanceof ArrayBuffer)return Ks(e,t);if(Ra(e))return Js(e,t);if(Va(e))return Ys(e,t);if(La(e)){let n=e.body;if(!n)throw Error(`Readable stream not available on Response`);return Ys(n,t)}throw Error(`makeIterator`)}var $s=`Cannot convert supplied data type`;function ec(e,t,n){if(t.text&&typeof e==`string`)return e;if(Oo(e)&&(e=e.buffer),Pa(e)){let n=Mo(e);return t.text&&!t.binary?new TextDecoder(`utf8`).decode(n):ko(n)}throw Error($s)}async function tc(e,t,n){if(typeof e==`string`||Pa(e))return ec(e,t,n);if(Ra(e)&&(e=await Jo(e)),La(e))return await Yo(e),t.binary?await e.arrayBuffer():await e.text();if(Va(e)&&(e=Qs(e,n)),Fa(e)||Ia(e))return xo(e);throw Error($s)}function nc(e,t){let n=ls(),r=e||n,i=r.fetch??r.core?.fetch;return typeof i==`function`?i:ja(i)?e=>ns(e,i):t?.fetch?t?.fetch:ns}function rc(e,t,n){if(n)return n;let r={fetch:nc(t,e),...e};if(r.url){let e=Uo(r.url);r.baseUrl=e,r.queryString=Ho(r.url),r.filename=No(e),r.baseUrl=Po(e)}return Array.isArray(r.loaders)||(r.loaders=null),r}function ic(e,t){if(e&&!Array.isArray(e))return e;let n;if(e&&(n=Array.isArray(e)?e:[e]),t&&t.loaders){let e=Array.isArray(t.loaders)?t.loaders:[t.loaders];n=n?[...n,...e]:e}return n&&n.length?n:void 0}async function ac(e,t,n,r){t&&!Array.isArray(t)&&!xs(t)&&(r=void 0,n=t,t=void 0),e=await e,n||={};let i=Go(e),a=ic(t,r),o=await Ds(e,a,n);if(!o)return null;let s=us(n,o,a,i);return r=rc({url:i,_parse:ac,loaders:a},s,r||null),await oc(o,e,s,r)}async function oc(e,t,n,r){if(mo(e),n=Ha(e.options,n),La(t)){let{ok:e,redirected:n,status:i,statusText:a,type:o,url:s}=t;r.response={headers:Object.fromEntries(t.headers.entries()),ok:e,redirected:n,status:i,statusText:a,type:o,url:s}}t=await tc(t,e,n);let i=e;if(i.parseTextSync&&typeof t==`string`)return i.parseTextSync(t,n,r);if(ho(e,n))return await go(e,t,n,r,ac);if(i.parseText&&typeof t==`string`)return await i.parseText(t,n,r);if(i.parse)return await i.parse(t,n,r);throw qa(!i.parseSync),Error(`${e.id} loader - no parser found and worker is disabled`)}async function sc(e,t,n,r){let i,a;!Array.isArray(t)&&!xs(t)?(i=[],a=t,r=void 0):(i=t,a=n);let o=nc(a),s=e;return typeof e==`string`&&(s=await o(e)),Ra(e)&&(s=await o(e)),typeof e==`string`&&(ds(a||{}).core?.baseUrl||(a={...a,core:{...a?.core,baseUrl:e}})),await ac(s,i,a)}var cc=`4.4.3`;function lc(e,t){if(!e)throw Error(t||`loader assertion failed.`)}var uc={self:typeof self<`u`&&self,window:typeof window<`u`&&window,global:typeof global<`u`&&global,document:typeof document<`u`&&document};uc.self||uc.window||uc.global,uc.window||uc.self||uc.global,uc.global||uc.self||uc.window,uc.document;var dc=!!(typeof process!=`object`||String(process)!==`[object process]`||process.browser),fc=typeof process<`u`&&process.version&&/v([0-9]*)/.exec(process.version);fc&&parseFloat(fc[1]);var pc=globalThis.loaders?.parseImageNode,mc=typeof Image<`u`,hc=typeof ImageBitmap<`u`,gc=dc?!0:!!pc;function _c(e){switch(e){case`auto`:return hc||mc||gc;case`imagebitmap`:return hc;case`image`:return mc;case`data`:return gc;default:throw Error(`@loaders.gl/images: image ${e} not supported in this environment`)}}function vc(){if(hc)return`imagebitmap`;if(mc)return`image`;if(gc)return`data`;throw Error(`Install '@loaders.gl/polyfills' to parse images under Node.js`)}function yc(e){let t=xc(e);if(!t)throw Error(`Not an image`);return t}function bc(e){switch(yc(e)){case`data`:return e;case`image`:case`imagebitmap`:let t=document.createElement(`canvas`),n=t.getContext(`2d`);if(!n)throw Error(`getImageData`);return t.width=e.width,t.height=e.height,n.drawImage(e,0,0),n.getImageData(0,0,e.width,e.height);default:throw Error(`getImageData`)}}function xc(e){return typeof ImageBitmap<`u`&&e instanceof ImageBitmap?`imagebitmap`:typeof Image<`u`&&e instanceof Image?`image`:e&&typeof e==`object`&&e.data&&e.width&&e.height?`data`:null}var Sc=/^data:image\/svg\+xml/,Cc=/\.svg((\?|#).*)?$/;function wc(e){return e&&(Sc.test(e)||Cc.test(e))}function Tc(e,t){if(wc(t)){let t=new TextDecoder().decode(e);try{typeof unescape==`function`&&typeof encodeURIComponent==`function`&&(t=unescape(encodeURIComponent(t)))}catch(e){throw Error(e.message)}return`data:image/svg+xml;base64,${btoa(t)}`}return Ec(e,t)}function Ec(e,t){if(wc(t))throw Error(`SVG cannot be parsed directly to imagebitmap`);return new Blob([new Uint8Array(e)])}async function Dc(e,t,n){let r=Tc(e,n),i=self.URL||self.webkitURL,a=typeof r!=`string`&&i.createObjectURL(r);try{return await Oc(a||r,t)}finally{a&&i.revokeObjectURL(a)}}async function Oc(e,t){let n=new Image;return n.src=e,t.image&&t.image.decode&&n.decode?(await n.decode(),n):await new Promise((e,t)=>{try{n.onload=()=>e(n),n.onerror=e=>{let n=e instanceof Error?e.message:`error`;t(Error(n))}}catch(e){t(e)}})}var kc=!0;async function Ac(e,t,n){let r;r=wc(n)?await Dc(e,t,n):Ec(e,n);let i=t&&t.imagebitmap;return await jc(r,i)}async function jc(e,t=null){if((Mc(t)||!kc)&&(t=null),t)try{return await createImageBitmap(e,t)}catch(e){console.warn(e),kc=!1}return await createImageBitmap(e)}function Mc(e){if(!e)return!0;for(let t in e)if(Object.prototype.hasOwnProperty.call(e,t))return!1;return!0}function Nc(e){return!Lc(e,`ftyp`,4)||!(e[8]&96)?null:Pc(e)}function Pc(e){switch(Fc(e,8,12).replace(`\0`,` `).trim()){case`avif`:case`avis`:return{extension:`avif`,mimeType:`image/avif`};default:return null}}function Fc(e,t,n){return String.fromCharCode(...e.slice(t,n))}function Ic(e){return[...e].map(e=>e.charCodeAt(0))}function Lc(e,t,n=0){let r=Ic(t);for(let t=0;t<r.length;++t)if(r[t]!==e[t+n])return!1;return!0}var Rc=!1,zc=!0;function Bc(e){let t=qc(e);return Hc(t)||Gc(t)||Uc(t)||Wc(t)||Vc(t)}function Vc(e){let t=Nc(new Uint8Array(e instanceof DataView?e.buffer:e));return t?{mimeType:t.mimeType,width:0,height:0}:null}function Hc(e){let t=qc(e);return t.byteLength>=24&&t.getUint32(0,Rc)===2303741511?{mimeType:`image/png`,width:t.getUint32(16,Rc),height:t.getUint32(20,Rc)}:null}function Uc(e){let t=qc(e);return t.byteLength>=10&&t.getUint32(0,Rc)===1195984440?{mimeType:`image/gif`,width:t.getUint16(6,zc),height:t.getUint16(8,zc)}:null}function Wc(e){let t=qc(e);return t.byteLength>=14&&t.getUint16(0,Rc)===16973&&t.getUint32(2,zc)===t.byteLength?{mimeType:`image/bmp`,width:t.getUint32(18,zc),height:t.getUint32(22,zc)}:null}function Gc(e){let t=qc(e);if(!(t.byteLength>=3&&t.getUint16(0,Rc)===65496&&t.getUint8(2)===255))return null;let{tableMarkers:n,sofMarkers:r}=Kc(),i=2;for(;i+9<t.byteLength;){let e=t.getUint16(i,Rc);if(r.has(e))return{mimeType:`image/jpeg`,height:t.getUint16(i+5,Rc),width:t.getUint16(i+7,Rc)};if(!n.has(e))return null;i+=2,i+=t.getUint16(i,Rc)}return null}function Kc(){let e=new Set([65499,65476,65484,65501,65534]);for(let t=65504;t<65520;++t)e.add(t);return{tableMarkers:e,sofMarkers:new Set([65472,65473,65474,65475,65477,65478,65479,65481,65482,65483,65485,65486,65487,65502])}}function qc(e){if(e instanceof DataView)return e;if(ArrayBuffer.isView(e))return new DataView(e.buffer);if(e instanceof ArrayBuffer)return new DataView(e);throw Error(`toDataView`)}async function Jc(e,t){let{mimeType:n}=Bc(e)||{},r=globalThis.loaders?.parseImageNode;return lc(r),await r(e,n)}async function Yc(e,t,n){t||={};let r=(t.image||{}).type||`auto`,{url:i}=n||{},a=Xc(r),o;switch(a){case`imagebitmap`:o=await Ac(e,t,i);break;case`image`:o=await Dc(e,t,i);break;case`data`:o=await Jc(e,t);break;default:lc(!1)}return r===`data`&&(o=bc(o)),o}function Xc(e){switch(e){case`auto`:case`data`:return vc();default:return _c(e),e}}var Zc={dataType:null,batchType:null,id:`image`,module:`images`,name:`Images`,version:cc,mimeTypes:[`image/png`,`image/jpeg`,`image/gif`,`image/webp`,`image/avif`,`image/bmp`,`image/vnd.microsoft.icon`,`image/svg+xml`],extensions:[`png`,`jpg`,`jpeg`,`gif`,`webp`,`bmp`,`ico`,`svg`,`avif`],parse:Yc,tests:[e=>!!Bc(new DataView(e))],options:{image:{type:`auto`,decode:!0}}},z=new D({id:`deck`}),Qc={};function $c(e){Qc=e}function el(e,t,n,r){z.level>0&&Qc[e]&&Qc[e].call(null,t,n,r)}function tl(e){let t=e[0],n=e[e.length-1];return t===`{`&&n===`}`||t===`[`&&n===`]`}var nl={dataType:null,batchType:null,id:`JSON`,name:`JSON`,module:``,version:``,options:{},extensions:[`json`,`geojson`],mimeTypes:[`application/json`,`application/geo+json`],testText:tl,parseTextSync:JSON.parse};function rl(){let e=`9.4.0`,t=globalThis.deck&&globalThis.deck.VERSION;if(t&&t!==e)throw Error(`deck.gl - multiple versions detected: ${t} vs ${e}`);return t||(z.log(1,`deck.gl ${e}`)(),globalThis.deck={...globalThis.deck,VERSION:e,version:e,log:z,_registerLoggers:$c},ws([nl,[Zc,{imagebitmap:{premultiplyAlpha:`none`}}]])),e}var il=rl(),al=`struct LayerUniforms {
  opacity: f32,
};

@group(0) @binding(auto)
var<uniform> layer: LayerUniforms;
`,ol=`layout(std140) uniform layerUniforms {
  uniform float opacity;
} layer;
`,sl={name:`layer`,source:al,vs:ol,fs:ol,getUniforms:e=>({opacity:e.opacity**(1/2.2)}),uniformTypes:{opacity:`f32`}},cl={name:`color`,dependencies:[],source:`

@must_use
fn deckgl_premultiplied_alpha(fragColor: vec4<f32>) -> vec4<f32> {
    return vec4(fragColor.rgb * fragColor.a, fragColor.a); 
};
`,getUniforms:e=>({})},ll=`const SMOOTH_EDGE_RADIUS: f32 = 0.5;

struct VertexGeometry {
  position: vec4<f32>,
  worldPosition: vec3<f32>,
  worldPositionAlt: vec3<f32>,
  normal: vec3<f32>,
  uv: vec2<f32>,
  pickingColor: vec3<f32>,
};

var<private> geometry_: VertexGeometry = VertexGeometry(
  vec4<f32>(0.0, 0.0, 1.0, 0.0),
  vec3<f32>(0.0, 0.0, 0.0),
  vec3<f32>(0.0, 0.0, 0.0),
  vec3<f32>(0.0, 0.0, 0.0),
  vec2<f32>(0.0, 0.0),
  vec3<f32>(0.0, 0.0, 0.0)
);

struct FragmentGeometry {
  uv: vec2<f32>,
};

var<private> fragmentGeometry: FragmentGeometry;

fn smoothedge(edge: f32, x: f32) -> f32 {
  return smoothstep(edge - SMOOTH_EDGE_RADIUS, edge + SMOOTH_EDGE_RADIUS, x);
}
`,ul=`#define SMOOTH_EDGE_RADIUS 0.5`,dl={name:`geometry`,source:ll,vs:`\
${ul}

struct VertexGeometry {
  vec4 position;
  vec3 worldPosition;
  vec3 worldPositionAlt;
  vec3 normal;
  vec2 uv;
  vec3 pickingColor;
} geometry = VertexGeometry(
  vec4(0.0, 0.0, 1.0, 0.0),
  vec3(0.0),
  vec3(0.0),
  vec3(0.0),
  vec2(0.0),
  vec3(0.0)
);
`,fs:`\
${ul}

struct FragmentGeometry {
  vec2 uv;
};
FragmentGeometry geometry;

float smoothedge(float edge, float x) {
  return smoothstep(edge - SMOOTH_EDGE_RADIUS, edge + SMOOTH_EDGE_RADIUS, x);
}
`},B;(function(e){e[e.Start=1]=`Start`,e[e.Move=2]=`Move`,e[e.End=4]=`End`,e[e.Cancel=8]=`Cancel`})(B||={});var fl;(function(e){e[e.None=0]=`None`,e[e.Left=1]=`Left`,e[e.Right=2]=`Right`,e[e.Up=4]=`Up`,e[e.Down=8]=`Down`,e[e.Horizontal=3]=`Horizontal`,e[e.Vertical=12]=`Vertical`,e[e.All=15]=`All`})(fl||={});var V;(function(e){e[e.Possible=1]=`Possible`,e[e.Began=2]=`Began`,e[e.Changed=4]=`Changed`,e[e.Ended=8]=`Ended`,e[e.Recognized=8]=`Recognized`,e[e.Cancelled=16]=`Cancelled`,e[e.Failed=32]=`Failed`})(V||={});var pl=`auto`,ml=`manipulation`,hl=`none`,gl=`pan-x`,_l=`pan-y`;function vl(e){if(e.includes(`none`))return hl;let t=e.includes(gl),n=e.includes(_l);return t&&n?hl:t||n?t?gl:_l:e.includes(`manipulation`)?ml:pl}var yl=class{constructor(e,t){this.actions=``,this.manager=e,this.set(t)}set(e){e===`compute`&&(e=this.compute()),this.manager.element&&(this.manager.element.style.touchAction=e,this.actions=e)}update(){this.set(this.manager.options.touchAction)}compute(){let e=[];for(let t of this.manager.recognizers)t.options.enable&&(e=e.concat(t.getTouchAction()));return vl(e.join(` `))}};function bl(e){return e.trim().split(/\s+/g)}function xl(e,t,n){if(e)for(let r of bl(t))e.addEventListener(r,n,!1)}function Sl(e,t,n){if(e)for(let r of bl(t))e.removeEventListener(r,n,!1)}function Cl(e){return(e.ownerDocument||e).defaultView}function wl(e,t){let n=e;for(;n;){if(n===t)return!0;n=n.parentNode}return!1}function Tl(e){let t=e.length;if(t===1)return{x:Math.round(e[0].clientX),y:Math.round(e[0].clientY)};let n=0,r=0,i=0;for(;i<t;)n+=e[i].clientX,r+=e[i].clientY,i++;return{x:Math.round(n/t),y:Math.round(r/t)}}function El(e){let t=[],n=0;for(;n<e.pointers.length;)t[n]={clientX:Math.round(e.pointers[n].clientX),clientY:Math.round(e.pointers[n].clientY)},n++;return{timeStamp:Date.now(),pointers:t,center:Tl(t),deltaX:e.deltaX,deltaY:e.deltaY}}function Dl(e,t){let n=t.x-e.x,r=t.y-e.y;return Math.sqrt(n*n+r*r)}function Ol(e,t){let n=t.clientX-e.clientX,r=t.clientY-e.clientY;return Math.sqrt(n*n+r*r)}function kl(e,t){let n=t.x-e.x,r=t.y-e.y;return Math.atan2(r,n)*180/Math.PI}function Al(e,t){let n=t.clientX-e.clientX,r=t.clientY-e.clientY;return Math.atan2(r,n)*180/Math.PI}function jl(e,t){return e===t?fl.None:Math.abs(e)>=Math.abs(t)?e<0?fl.Left:fl.Right:t<0?fl.Up:fl.Down}function Ml(e,t){let n=t.center,r=e.offsetDelta,i=e.prevDelta,a=e.prevInput;return(t.eventType===B.Start||a?.eventType===B.End)&&(i=e.prevDelta={x:a?.deltaX||0,y:a?.deltaY||0},r=e.offsetDelta={x:n.x,y:n.y}),{deltaX:i.x+(n.x-r.x),deltaY:i.y+(n.y-r.y)}}function Nl(e,t,n){return{x:t/e||0,y:n/e||0}}function Pl(e,t){return Ol(t[0],t[1])/Ol(e[0],e[1])}function Fl(e,t){return Al(t[1],t[0])-Al(e[1],e[0])}function Il(e,t){let n=e.lastInterval||t,r=t.timeStamp-n.timeStamp,i,a,o,s;if(t.eventType!==B.Cancel&&(r>25||n.velocity===void 0)){let c=t.deltaX-n.deltaX,l=t.deltaY-n.deltaY,u=Nl(r,c,l);a=u.x,o=u.y,i=Math.abs(u.x)>Math.abs(u.y)?u.x:u.y,s=jl(c,l),e.lastInterval=t}else i=n.velocity,a=n.velocityX,o=n.velocityY,s=n.direction;t.velocity=i,t.velocityX=a,t.velocityY=o,t.direction=s}function Ll(e,t){return`pointerId`in e?e.pointerId:t}function Rl(e,t){e.movementOrigin=new Map(t.map((e,t)=>[Ll(e,t),{clientX:e.clientX,clientY:e.clientY}])),e.firstMovementTime=void 0}function zl(e,t){let n=t.pointers.map(Ll);if(e.movementOrigin?.size===n.length&&n.every(t=>e.movementOrigin.has(t))||Rl(e,t.pointers),t.distancePerPointer=t.pointers.map((t,r)=>Ol(e.movementOrigin.get(n[r]),t)),t.eventType&B.Move&&t.distancePerPointer.some(e=>e>0)&&(e.firstMovementTime??=t.timeStamp),t.movementDeltaTime=e.firstMovementTime===void 0?0:t.timeStamp-e.firstMovementTime,t.eventType&(B.End|B.Cancel)){let r=t.changedPointers.map(e=>Ll(e,t.pointers.indexOf(e)));Rl(e,t.pointers.filter((e,t)=>!r.includes(n[t])))}}function Bl(e,t){let{session:n}=e,{pointers:r}=t,{length:i}=r;n.firstInput||=El(t),i>1&&!n.firstMultiple?n.firstMultiple=El(t):i===1&&(n.firstMultiple=!1);let{firstInput:a,firstMultiple:o}=n,s=o?o.center:a.center,c=t.center=Tl(r);t.timeStamp=Date.now(),t.deltaTime=t.timeStamp-a.timeStamp,zl(n,t),t.angle=kl(s,c),t.distance=Dl(s,c);let{deltaX:l,deltaY:u}=Ml(n,t);t.deltaX=l,t.deltaY=u,t.offsetDirection=jl(t.deltaX,t.deltaY);let d=Nl(t.deltaTime,t.deltaX,t.deltaY);t.overallVelocityX=d.x,t.overallVelocityY=d.y,t.overallVelocity=Math.abs(d.x)>Math.abs(d.y)?d.x:d.y,t.scale=o?Pl(o.pointers,r):1,t.rotation=o?Fl(o.pointers,r):0,t.maxPointers=n.prevInput?t.pointers.length>n.prevInput.maxPointers?t.pointers.length:n.prevInput.maxPointers:t.pointers.length;let f=e.element;return wl(t.srcEvent.target,f)&&(f=t.srcEvent.target),t.target=f,Il(n,t),t}function Vl(e,t,n){let r=n.pointers.length,i=n.changedPointers.length,a=t&B.Start&&r-i===0,o=t&(B.End|B.Cancel)&&r-i===0;n.isFirst=!!a,n.isFinal=!!o,a&&(e.session={}),n.eventType=t;let s=Bl(e,n);e.emit(`hammer.input`,s),e.recognize(s),e.session.prevInput=s}var Hl=class{constructor(e){this.evEl=``,this.evWin=``,this.evTarget=``,this.domHandler=e=>{this.manager.options.enable&&this.handler(e)},this.manager=e,this.element=e.element,this.target=e.options.inputTarget||e.element}callback(e,t){Vl(this.manager,e,t)}init(){xl(this.element,this.evEl,this.domHandler),xl(this.target,this.evTarget,this.domHandler),xl(Cl(this.element),this.evWin,this.domHandler)}destroy(){Sl(this.element,this.evEl,this.domHandler),Sl(this.target,this.evTarget,this.domHandler),Sl(Cl(this.element),this.evWin,this.domHandler)}},Ul={pointerdown:B.Start,pointermove:B.Move,pointerup:B.End,pointercancel:B.Cancel,pointerout:B.Cancel},Wl=`pointerdown`,Gl=`pointermove pointerup pointercancel`,Kl=class extends Hl{constructor(e){super(e),this.evEl=Wl,this.evWin=Gl,this.store=this.manager.session.pointerEvents=[],this.init()}handler(e){let{store:t}=this,n=!1,r=Ul[e.type],i=e.pointerType,a=i===`touch`,o=t.findIndex(t=>t.pointerId===e.pointerId);r&B.Start&&(e.buttons||a)?o<0&&(t.push(e),o=t.length-1):r&(B.End|B.Cancel)&&(n=!0),!(o<0)&&(t[o]=e,this.callback(r,{pointers:t,changedPointers:[e],eventType:r,pointerType:i,srcEvent:e}),n&&t.splice(o,1))}},ql=[``,`webkit`,`Moz`,`MS`,`ms`,`o`];function Jl(e,t){let n=t[0].toUpperCase()+t.slice(1);for(let r of ql){let i=r?r+n:t;if(i in e)return i}}var Yl=1,Xl=2,Zl={touchAction:`compute`,enable:!0,inputTarget:null,cssProps:{userSelect:`none`,userDrag:`none`,touchCallout:`none`,tapHighlightColor:`rgba(0,0,0,0)`}},Ql=class{constructor(e,t){this.options={...Zl,...t,cssProps:{...Zl.cssProps,...t.cssProps},inputTarget:t.inputTarget||e},this.handlers={},this.session={},this.recognizers=[],this.oldCssProps={},this.element=e,this.input=new Kl(this),this.touchAction=new yl(this,this.options.touchAction),this.toggleCssProps(!0)}set(e){return Object.assign(this.options,e),e.touchAction&&this.touchAction.update(),e.inputTarget&&(this.input.destroy(),this.input.target=e.inputTarget,this.input.init()),this}stop(e){this.session.stopped=e?Xl:Yl}recognize(e){let{session:t}=this;if(t.stopped)return;this.session.prevented&&e.srcEvent.preventDefault();let n,{recognizers:r}=this,{curRecognizer:i}=t;(!i||i&&i.state&V.Recognized)&&(i=t.curRecognizer=null);let a=0;for(;a<r.length;)n=r[a],t.stopped!==Xl&&(!i||n===i||n.canRecognizeWith(i))?n.recognize(e):n.reset(),!i&&n.state&(V.Began|V.Changed|V.Ended)&&(i=t.curRecognizer=n),a++}get(e){let{recognizers:t}=this;for(let n=0;n<t.length;n++)if(t[n].options.event===e)return t[n];return null}add(e){if(Array.isArray(e)){for(let t of e)this.add(t);return this}let t=this.get(e.options.event);return t&&this.remove(t),this.recognizers.push(e),e.manager=this,this.touchAction.update(),e}remove(e){if(Array.isArray(e)){for(let t of e)this.remove(t);return this}let t=typeof e==`string`?this.get(e):e;if(t){let{recognizers:e}=this,n=e.indexOf(t);n!==-1&&(e.splice(n,1),this.touchAction.update())}return this}on(e,t){if(!e||!t)return;let{handlers:n}=this;for(let r of bl(e))n[r]=n[r]||[],n[r].push(t)}off(e,t){if(!e)return;let{handlers:n}=this;for(let r of bl(e))t?n[r]&&n[r].splice(n[r].indexOf(t),1):delete n[r]}emit(e,t){let n=this.handlers[e]&&this.handlers[e].slice();if(!n||!n.length)return;let r=t;r.type=e,r.preventDefault=function(){t.srcEvent.preventDefault()};let i=0;for(;i<n.length;)n[i](r),i++}destroy(){this.toggleCssProps(!1),this.handlers={},this.session={},this.input.destroy(),this.element=null}toggleCssProps(e){let{element:t}=this;if(t){for(let[n,r]of Object.entries(this.options.cssProps)){let i=Jl(t.style,n);e?(this.oldCssProps[i]=t.style[i],t.style[i]=r):t.style[i]=this.oldCssProps[i]||``}e||(this.oldCssProps={})}}},$l=1;function eu(){return $l++}function tu(e){return e&V.Cancelled?`cancel`:e&V.Ended?`end`:e&V.Changed?`move`:e&V.Began?`start`:``}var nu=class{constructor(e){this.options=e,this.id=eu(),this.state=V.Possible,this.simultaneous={},this.requireFail=[]}set(e){return Object.assign(this.options,e),this.manager.touchAction.update(),this}recognizeWith(e){if(Array.isArray(e)){for(let t of e)this.recognizeWith(t);return this}let t;if(typeof e==`string`){if(t=this.manager.get(e),!t)throw Error(`Cannot find recognizer ${e}`)}else t=e;let{simultaneous:n}=this;return n[t.id]||(n[t.id]=t,t.recognizeWith(this)),this}dropRecognizeWith(e){if(Array.isArray(e)){for(let t of e)this.dropRecognizeWith(t);return this}let t;return t=typeof e==`string`?this.manager.get(e):e,t&&delete this.simultaneous[t.id],this}requireFailure(e){if(Array.isArray(e)){for(let t of e)this.requireFailure(t);return this}let t;if(typeof e==`string`){if(t=this.manager.get(e),!t)throw Error(`Cannot find recognizer ${e}`)}else t=e;let{requireFail:n}=this;return n.indexOf(t)===-1&&(n.push(t),t.requireFailure(this)),this}dropRequireFailure(e){if(Array.isArray(e)){for(let t of e)this.dropRequireFailure(t);return this}let t;if(t=typeof e==`string`?this.manager.get(e):e,t){let e=this.requireFail.indexOf(t);e>-1&&this.requireFail.splice(e,1)}return this}hasRequireFailures(){return!!this.requireFail.find(e=>e.options.enable)}canRecognizeWith(e){return!!this.simultaneous[e.id]}emit(e){if(!e)return;let{state:t}=this;t<V.Ended&&this.manager.emit(this.options.event+tu(t),e),this.manager.emit(this.options.event,e),e.additionalEvent&&this.manager.emit(e.additionalEvent,e),t>=V.Ended&&this.manager.emit(this.options.event+tu(t),e)}tryEmit(e){this.canEmit()?this.emit(e):this.state=V.Failed}canEmit(){let e=0;for(;e<this.requireFail.length;){if(!(this.requireFail[e].state&(V.Failed|V.Possible)))return!1;e++}return!0}recognize(e){let t={...e};if(!this.options.enable){this.reset(),this.state=V.Failed;return}this.state&(V.Recognized|V.Cancelled|V.Failed)&&(this.state=V.Possible),this.state=this.process(t),this.state&(V.Began|V.Changed|V.Ended|V.Cancelled)&&this.tryEmit(t)}getEventNames(){return[this.options.event]}reset(){}};function ru(e){return Math.abs(((e+180)%360+360)%360-180)}function iu(e,t){return(t.distance===void 0||e.distance>=t.distance)&&(t.distancePerPointer===void 0||e.distancePerPointer.length>0&&e.distancePerPointer.every(e=>e>=t.distancePerPointer))&&(t.movementDeltaTime===void 0||e.movementDeltaTime>=t.movementDeltaTime)&&(t.rotation===void 0||ru(e.rotation)>=t.rotation)&&(t.scale===void 0||Math.abs(e.scale-1)>=t.scale)}var au=class extends nu{attrTest(e){let t=this.options.pointers;return t===0||e.pointers.length===t}coherentTest(e){let t=this.options.coherent;return!t?.length||t.some(t=>iu(e,t))}process(e){let{state:t}=this,{eventType:n}=e,r=t&(V.Began|V.Changed),i=this.attrTest(e);return r&&(n&B.Cancel||!i)?t|V.Cancelled:r||i?n&B.End?t|V.Ended:t&V.Began?t|V.Changed:V.Began:V.Failed}},ou=[``,`start`,`move`,`end`,`cancel`],su=class extends nu{constructor(e={}){super({enable:!0,event:`doubleclickdrag`,pointers:1,interval:500,time:350,threshold:28,dragThreshold:1,pixelsPerScale:120,...e}),this._tapStart=null,this._lastTap=null,this._drag=null,this._emittedStart=!1}getTouchAction(){return[ml]}getEventNames(){return ou.map(e=>this.options.event+e)}process(e){let{options:t}=this;return e.pointers.length===t.pointers?e.eventType&B.Start?this._handleStart(e):e.eventType&B.Move?this._handleMove(e):e.eventType&B.Cancel?this._handleEnd(e,!0):e.eventType&B.End?this._handleEnd(e,!1):V.Failed:(this.reset(),V.Failed)}reset(){this._tapStart=null,this._lastTap=null,this._drag=null,this._emittedStart=!1}emit(e){if(e){if(this.state===V.Began){if(!this._drag?.active||this._emittedStart)return;this._emittedStart=!0,this.manager.emit(`${this.options.event}start`,e),this.manager.emit(this.options.event,e);return}if(this.state===V.Changed){if(!this._emittedStart)return;this.manager.emit(`${this.options.event}move`,e),this.manager.emit(this.options.event,e);return}if(this.state===V.Ended){if(!this._emittedStart)return;this.manager.emit(this.options.event,e),this.manager.emit(`${this.options.event}end`,e),this._emittedStart=!1;return}if(this.state===V.Cancelled){if(!this._emittedStart)return;this.manager.emit(this.options.event,e),this.manager.emit(`${this.options.event}cancel`,e),this._emittedStart=!1}}}_handleStart(e){let t=this._getPointerId(e);return this._lastTap&&this._isTapMatch(e,this._lastTap)?(this._tapStart=null,this._lastTap=null,this._drag={startCenter:e.center,pointerId:t,active:!1},this._emittedStart=!1,V.Began):(this._tapStart={center:e.center,timeStamp:e.timeStamp,pointerId:t},this._lastTap=null,this._drag=null,this._emittedStart=!1,V.Failed)}_handleMove(e){if(!this._drag||!this._isSamePointer(e,this._drag.pointerId))return V.Failed;let t=this._drag.startCenter.y-e.center.y;return!this._drag.active&&Math.abs(t)<this.options.dragThreshold?V.Began:(this._drag.active=!0,e.scale=2**(t/this.options.pixelsPerScale),this._emittedStart?V.Changed:V.Began)}_handleEnd(e,t){if(this._drag&&this._isSamePointer(e,this._drag.pointerId)){let{active:n,startCenter:r}=this._drag;return this._drag=null,this._tapStart=null,this._lastTap=null,n?(e.scale=2**((r.y-e.center.y)/this.options.pixelsPerScale),t?V.Cancelled:V.Ended):(this._emittedStart=!1,V.Failed)}return!this._tapStart||!this._isSamePointer(e,this._tapStart.pointerId)?(t&&this.reset(),V.Failed):(this._isValidTap(e)?this._lastTap={center:e.center,timeStamp:e.timeStamp,pointerId:this._tapStart.pointerId}:this._lastTap=null,this._tapStart=null,V.Failed)}_isTapMatch(e,t){return e.timeStamp-t.timeStamp<=this.options.interval&&Dl(e.center,t.center)<=this.options.threshold}_isValidTap(e){return e.deltaTime<=this.options.time&&e.distance<=this.options.threshold}_getPointerId(e){return`pointerId`in e.srcEvent?e.srcEvent.pointerId:null}_isSamePointer(e,t){return t===null||this._getPointerId(e)===t}},cu=class extends nu{constructor(e={}){super({enable:!0,event:`tap`,pointers:1,taps:1,interval:300,time:250,threshold:9,posThreshold:10,...e}),this.pTime=null,this.pCenter=null,this._timer=null,this._input=null,this.count=0}getTouchAction(){return[ml]}process(e){let{options:t}=this,n=e.pointers.length===t.pointers,r=e.distance<t.threshold,i=e.deltaTime<t.time;if(this.reset(),e.eventType&B.Start&&this.count===0)return this.failTimeout();if(r&&i&&n){if(e.eventType!==B.End)return this.failTimeout();let n=!this.pTime||e.timeStamp-this.pTime<t.interval,r=!this.pCenter||Dl(this.pCenter,e.center)<t.posThreshold;if(this.pTime=e.timeStamp,this.pCenter=e.center,!r||!n?this.count=1:this.count+=1,this._input=e,this.count%t.taps===0)return this.hasRequireFailures()?(this._timer=setTimeout(()=>{this.state=V.Recognized,this.tryEmit(this._input)},t.interval),V.Began):V.Recognized}return V.Failed}failTimeout(){return this._timer=setTimeout(()=>{this.state=V.Failed},this.options.interval),V.Failed}reset(){clearTimeout(this._timer)}emit(e){this.state===V.Recognized&&(e.tapCount=this.count,this.manager.emit(this.options.event,e))}},lu=class extends au{constructor(){super(...arguments),this.wheelSession=null,this.wheelSessionUnsubscribe=null,this.handleWheelSessionEvent=e=>{e.device===`trackpad`&&this.handleTrackpadEvent(e)}}set(e){let{wheelSession:t,...n}=e;return t&&t!==this.wheelSession&&(this.wheelSessionUnsubscribe?.(),this.wheelSessionUnsubscribe=null,this.wheelSession=t),super.set(n),this.updateWheelSessionSubscription(),this}getTrackpadInput(e,t={}){let{srcEvent:n}=e,r=t.deltaX??e.deltaX,i=t.deltaY??e.deltaY,a=jl(r,i),o=Math.sqrt(e.deltaX*e.deltaX+e.deltaY*e.deltaY),s=n;return{pointers:[s,s],changedPointers:[s,s],pointerType:`trackpad`,srcEvent:s,eventType:e.eventType,timeStamp:e.timeStamp,deltaTime:e.deltaTime,center:e.center,deltaX:r,deltaY:i,angle:Math.atan2(i,r)*180/Math.PI,distance:Math.sqrt(r*r+i*i),distancePerPointer:[o,o],movementDeltaTime:e.deltaTime,scale:1,rotation:0,direction:a,offsetDirection:a,velocity:e.velocity,velocityX:e.velocityX,velocityY:e.velocityY,overallVelocity:e.overallVelocity,overallVelocityX:e.overallVelocityX,overallVelocityY:e.overallVelocityY,maxPointers:2,target:n.target||this.manager.element,additionalEvent:``,...t}}updateWheelSessionSubscription(){let e=!!(this.wheelSession&&this.options.enable&&this.options.trackpad&&this.options.pointers===2);e&&!this.wheelSessionUnsubscribe?this.wheelSessionUnsubscribe=this.wheelSession.on(this.handleWheelSessionEvent):!e&&this.wheelSessionUnsubscribe&&(this.wheelSessionUnsubscribe(),this.wheelSessionUnsubscribe=null)}},uu=[``,`start`,`move`,`end`,`cancel`,`up`,`down`,`left`,`right`],du=class extends lu{constructor(e={}){super({enable:!0,pointers:1,event:`pan`,threshold:10,direction:fl.All,trackpad:!1,coherent:[],...e}),this.trackpadGesture=!1,this.pX=null,this.pY=null}getTouchAction(){let{options:{direction:e}}=this,t=[];return e&fl.Horizontal&&t.push(_l),e&fl.Vertical&&t.push(gl),t}getEventNames(){return uu.map(e=>this.options.event+e)}directionTest(e){let{options:t}=this,n=!0,{distance:r}=e,{direction:i}=e,a=e.deltaX,o=e.deltaY;return i&t.direction||(t.direction&fl.Horizontal?(i=a===0?fl.None:a<0?fl.Left:fl.Right,n=a!==this.pX,r=Math.abs(e.deltaX)):(i=o===0?fl.None:o<0?fl.Up:fl.Down,n=o!==this.pY,r=Math.abs(e.deltaY))),e.direction=i,n&&r>t.threshold&&!!(i&t.direction)}attrTest(e){let t=!!(this.state&V.Began),n=!(this.options.coherent?.length&&e.eventType&(B.End|B.Cancel));return super.attrTest(e)&&(t||n&&this.coherentTest(e)&&this.directionTest(e))}emit(e){this.pX=e.deltaX,this.pY=e.deltaY;let t=fl[e.direction].toLowerCase();t&&(e.additionalEvent=this.options.event+t),super.emit(e)}handleTrackpadEvent(e){e.isFirst&&(this.trackpadGesture=!e.srcEvent.ctrlKey,!this.trackpadGesture&&this.state&(V.Recognized|V.Cancelled|V.Failed)&&(this.state=V.Possible)),this.trackpadGesture&&(this.recognize(this.getTrackpadInput(e,{deltaX:-e.deltaX,deltaY:-e.deltaY,velocity:-e.velocity,velocityX:-e.velocityX,velocityY:-e.velocityY,overallVelocity:-e.overallVelocity,overallVelocityX:-e.overallVelocityX,overallVelocityY:-e.overallVelocityY})),e.isFinal&&(this.trackpadGesture=!1))}},fu=[``,`start`,`move`,`end`,`cancel`,`in`,`out`],pu=class extends lu{constructor(e={}){super({enable:!0,event:`pinch`,threshold:0,pointers:2,trackpad:!1,coherent:[],...e}),this.trackpadGesture=!1}getTouchAction(){return[hl]}getEventNames(){return fu.map(e=>this.options.event+e)}attrTest(e){let t=!!this.options.coherent?.length,n=!!(this.state&V.Began),r=!(t&&e.eventType&(B.End|B.Cancel));return super.attrTest(e)&&(n||r&&(t?this.coherentTest(e):Math.abs(e.scale-1)>this.options.threshold))}emit(e){if(e.scale!==1){let t=e.scale<1?`in`:`out`;e.additionalEvent=this.options.event+t}super.emit(e)}handleTrackpadEvent(e){e.isFirst&&(this.trackpadGesture=e.srcEvent.ctrlKey,!this.trackpadGesture&&this.state&(V.Recognized|V.Cancelled|V.Failed)&&(this.state=V.Possible)),this.trackpadGesture&&(this.recognize(this.getTrackpadInput(e,{deltaX:0,deltaY:0,velocity:0,velocityX:0,velocityY:0,overallVelocity:0,overallVelocityX:0,overallVelocityY:0,scale:Math.exp(-e.deltaY/100)})),e.isFinal&&(this.trackpadGesture=!1))}},mu=class{constructor(e,t,n){this.element=e,this.callback=t,this.options=n}listen(e,t){t?this.element.addEventListener(e,this.handleEvent,{passive:!1}):this.element.removeEventListener(e,this.handleEvent)}},hu=(typeof navigator<`u`&&navigator.userAgent?navigator.userAgent.toLowerCase():``).indexOf(`firefox`)!==-1,gu=40,_u=.25,vu=class extends mu{constructor(e,t,n){n.enable=n.enable??!1,super(e,t,n),this.handleEvent=e=>{if(!this.options.enable)return;let t=e.deltaY;globalThis.WheelEvent&&(hu&&e.deltaMode===globalThis.WheelEvent.DOM_DELTA_PIXEL&&(t/=globalThis.devicePixelRatio),e.deltaMode===globalThis.WheelEvent.DOM_DELTA_LINE&&(t*=gu)),e.shiftKey&&t&&(t*=_u),this.callback({type:`wheel`,center:{x:e.clientX,y:e.clientY},delta:-t,device:this.options.wheelSession?.device??`unknown`,srcEvent:e,pointerType:`mouse`,target:e.target})},n.enable&&(this.wheelSessionUnsubscribe=this.options.wheelSession?.on(()=>{}),this.listen(`wheel`,!0))}destroy(){this.listen(`wheel`,!1),this.wheelSessionUnsubscribe?.(),this.wheelSessionUnsubscribe=void 0}enableEventType(e,t){e===`wheel`&&this.options.enable!==t&&(this.options.enable=t,t&&!this.wheelSessionUnsubscribe&&(this.wheelSessionUnsubscribe=this.options.wheelSession?.on(()=>{})),this.listen(`wheel`,t),t||(this.wheelSessionUnsubscribe?.(),this.wheelSessionUnsubscribe=void 0))}},yu=4.000244140625,bu=40,xu=0,Su=1,Cu=40,wu=40,Tu=120,Eu={classificationDelay:32,endDelay:80},Du=class{constructor(e,t={}){this.subscriptions=new Map,this.session=null,this.classificationTimer=null,this.endTimer=null,this.pressedControlKeys=new Set,this.listeningForControlKeys=!1,this.handleEvent=e=>{if(!this.hasSubscribers)return`unknown`;let t=ku(e,this.pressedControlKeys.size>0),n=this.session;if(n&&t.timeStamp-n.lastTimeStamp>=this.options.endDelay){if(this.end(),!this.hasSubscribers)return`unknown`;n=null}n?(this.scheduleEnd(),this.addSample(n,t)):(n=this.startPendingSession(t),this.scheduleEnd());let{device:r}=n;return r===`unknown`&&(r=Au(n.samples,!1),r!==`unknown`&&this.begin(n,r)),r},this.finishClassification=()=>{if(this.classificationTimer=null,!this.session||this.session.device!==`unknown`)return;let e=this.session,t=Au(e.samples,!0);this.begin(e,t===`unknown`?`mouse`:t)},this.end=()=>{if(!this.session)return;if(this.session.device===`unknown`){let e=this.session,t=Au(e.samples,!0);this.begin(e,t===`unknown`?`mouse`:t)}if(!this.session)return;let e=this.session;this.emit(B.End,e.lastEvent),this.reset()},this.handleKeyDown=e=>{e.key===`Control`&&this.pressedControlKeys.add(e.code||e.key)},this.handleKeyUp=e=>{e.key===`Control`&&(e.code?this.pressedControlKeys.delete(e.code):this.pressedControlKeys.clear())},this.handleWindowBlur=()=>{this.pressedControlKeys.clear()},this.element=e,this.options={...Eu,...t},this.element?.addEventListener(`wheel`,this.handleEvent,{passive:!0})}get hasSubscribers(){return this.subscriptions.size>0}get device(){return this.session?.device??`unknown`}on(e){let t={listener:e};return this.subscriptions.set(e,t),this.updateControlKeyEventListeners(),()=>{this.subscriptions.get(e)===t&&this.off(e)}}off(e){this.subscriptions.delete(e),this.updateControlKeyEventListeners(),this.hasSubscribers||this.reset()}cancel(){let e=this.session;e&&e.device!==`unknown`&&this.emit(B.Cancel,e.lastEvent),this.reset()}destroy(){this.cancel(),this.subscriptions.clear(),this.updateControlKeyEventListeners(),this.element?.removeEventListener(`wheel`,this.handleEvent)}startPendingSession(e){let t={samples:[e],device:`unknown`,firstTimeStamp:e.timeStamp,lastTimeStamp:e.timeStamp,totalDeltaX:e.deltaX,totalDeltaY:e.deltaY,velocityX:0,velocityY:0,lastEvent:e.event};return this.session=t,this.classificationTimer=globalThis.setTimeout(this.finishClassification,this.options.classificationDelay),t}addSample(e,t){if(e.samples.push(t),e.lastTimeStamp=t.timeStamp,e.lastEvent=t.event,e.totalDeltaX+=t.deltaX,e.totalDeltaY+=t.deltaY,e.device!==`unknown`){let n=e.samples[e.samples.length-2],r=t.timeStamp-n.timeStamp;e.velocityX=r>0?t.deltaX/r:0,e.velocityY=r>0?t.deltaY/r:0,this.emit(B.Move,t.event,{velocityX:e.velocityX,velocityY:e.velocityY})}}begin(e,t){e.device=t,this.clearClassificationTimer(),this.emit(B.Start,e.samples[0].event);let n=e.lastTimeStamp-e.firstTimeStamp;e.velocityX=n>0?e.totalDeltaX/n:0,e.velocityY=n>0?e.totalDeltaY/n:0,this.emit(B.Move,e.lastEvent,{velocityX:e.velocityX,velocityY:e.velocityY})}scheduleEnd(){this.clearEndTimer(),this.endTimer=globalThis.setTimeout(this.end,this.options.endDelay)}emit(e,t,n){let r=this.session;if(!r||r.device===`unknown`)return;let i=e===B.Start,a=e===B.End||e===B.Cancel,o=i?r.firstTimeStamp:r.lastTimeStamp,s=i?0:Math.max(0,o-r.firstTimeStamp),c=i?0:r.totalDeltaX,l=i?0:r.totalDeltaY,u=s>0?c/s:0,d=s>0?l/s:0,f=i?0:n?.velocityX??r.velocityX,p=i?0:n?.velocityY??r.velocityY,m={eventType:e,device:r.device,srcEvent:t,timeStamp:o,center:{x:t.clientX,y:t.clientY},deltaX:c,deltaY:l,deltaTime:s,velocity:Math.abs(f)>Math.abs(p)?f:p,velocityX:f,velocityY:p,overallVelocity:Math.abs(u)>Math.abs(d)?u:d,overallVelocityX:u,overallVelocityY:d,isFirst:i,isFinal:a};for(let{listener:e}of[...this.subscriptions.values()])e(m)}reset(){this.clearClassificationTimer(),this.clearEndTimer(),this.session=null}clearClassificationTimer(){this.classificationTimer!==null&&(globalThis.clearTimeout(this.classificationTimer),this.classificationTimer=null)}clearEndTimer(){this.endTimer!==null&&(globalThis.clearTimeout(this.endTimer),this.endTimer=null)}updateControlKeyEventListeners(){let e=this.hasSubscribers,t=Ou();!t||e===this.listeningForControlKeys||(this.listeningForControlKeys=e,e?(t.addEventListener(`keydown`,this.handleKeyDown,!0),t.addEventListener(`keyup`,this.handleKeyUp,!0),t.addEventListener(`blur`,this.handleWindowBlur)):(t.removeEventListener(`keydown`,this.handleKeyDown,!0),t.removeEventListener(`keyup`,this.handleKeyUp,!0),t.removeEventListener(`blur`,this.handleWindowBlur),this.pressedControlKeys.clear()))}};function Ou(){return typeof window<`u`?window:globalThis.document?.defaultView}function ku(e,t){let n=e.deltaX,r=e.deltaY;return e.deltaMode===Su&&(n*=bu,r*=bu),{event:e,timeStamp:e.timeStamp,deltaX:n,deltaY:r,isControlKeyDown:t}}function Au(e,t){return e.some(({event:e,isControlKeyDown:t})=>e.ctrlKey&&!t)?`trackpad`:e.some(({event:e})=>e.deltaMode!==xu)||e.some(ju)||e.every(({event:e})=>{let t=e.wheelDelta;return t!==void 0&&Math.abs(t)%40==0})?`mouse`:e.some(({deltaX:e})=>e!==0)||e.length>1&&Mu(e)?`trackpad`:t?`mouse`:`unknown`}function ju({event:e,deltaX:t,deltaY:n}){if(t!==0||n===0)return!1;let r=Math.abs(n/yu);if(Number.isInteger(r))return!0;let i=e.wheelDelta;return typeof i==`number`&&i!==0&&i%Tu===0}function Mu(e){for(let t=0;t<e.length;t++){let n=e[t];if(Math.abs(n.deltaX)>wu||Math.abs(n.deltaY)>wu||t>0&&n.timeStamp-e[t-1].timeStamp>Cu)return!1}return!0}var Nu=[`mousedown`,`mousemove`,`mouseup`,`mouseover`,`mouseout`,`mouseenter`,`mouseleave`],Pu=class extends mu{constructor(e,t,n){super(e,t,{enable:!0,...n}),this.handleEvent=e=>{this.handleOverEvent(e),this.handleOutEvent(e),this.handleEnterEvent(e),this.handleLeaveEvent(e),this.handleMoveEvent(e)},this.pressed=!1;let{enable:r=!1}=this.options;this.enableMoveEvent=r,this.enableLeaveEvent=r,this.enableEnterEvent=r,this.enableOutEvent=r,this.enableOverEvent=r,r&&Nu.forEach(e=>this.listen(e,!0))}destroy(){Nu.forEach(e=>this.listen(e,!1))}enableEventType(e,t){switch(e){case`pointermove`:this.enableMoveEvent!==t&&(this.enableMoveEvent=t,this.listen(`mousedown`,t),this.listen(`mousemove`,t),this.listen(`mouseup`,t));break;case`pointerover`:this.enableOverEvent!==t&&(this.enableOverEvent=t,this.listen(`mouseover`,t));break;case`pointerout`:this.enableOutEvent!==t&&(this.enableOutEvent=t,this.listen(`mouseout`,t));break;case`pointerenter`:this.enableEnterEvent!==t&&(this.enableEnterEvent=t,this.listen(`mouseenter`,t));break;case`pointerleave`:this.enableLeaveEvent!==t&&(this.enableLeaveEvent=t,this.listen(`mouseleave`,t));break;default:}}handleOverEvent(e){this.enableOverEvent&&e.type===`mouseover`&&this._emit(`pointerover`,e)}handleOutEvent(e){this.enableOutEvent&&e.type===`mouseout`&&this._emit(`pointerout`,e)}handleEnterEvent(e){this.enableEnterEvent&&e.type===`mouseenter`&&this._emit(`pointerenter`,e)}handleLeaveEvent(e){this.enableLeaveEvent&&e.type===`mouseleave`&&this._emit(`pointerleave`,e)}handleMoveEvent(e){if(this.enableMoveEvent)switch(e.type){case`mousedown`:e.button>=0&&(this.pressed=!0);break;case`mousemove`:e.buttons===0&&(this.pressed=!1),this.pressed||this._emit(`pointermove`,e);break;case`mouseup`:this.pressed=!1;break;default:}}_emit(e,t){this.callback({type:e,center:{x:t.clientX,y:t.clientY},srcEvent:t,pointerType:`mouse`,target:t.target})}},Fu=[`keydown`,`keyup`],Iu=class extends mu{constructor(e,t,n){super(e,t,{enable:!0,tabIndex:0,...n}),this.handleEvent=e=>{let t=e.target||e.srcElement;t.tagName===`INPUT`&&t.type===`text`||t.tagName===`TEXTAREA`||(this.enableDownEvent&&e.type===`keydown`&&this.callback({type:`keydown`,srcEvent:e,key:e.key,target:e.target}),this.enableUpEvent&&e.type===`keyup`&&this.callback({type:`keyup`,srcEvent:e,key:e.key,target:e.target}))};let{enable:r=!1}=this.options;this.enableDownEvent=r,this.enableUpEvent=r,e.tabIndex=this.options.tabIndex,e.style.outline=`none`,r&&Fu.forEach(e=>this.listen(e,!0))}destroy(){Fu.forEach(e=>this.listen(e,!1))}enableEventType(e,t){e===`keydown`&&this.enableDownEvent!==t&&(this.enableDownEvent=t,this.listen(e,t)),e===`keyup`&&this.enableUpEvent!==t&&(this.enableUpEvent=t,this.listen(e,t))}},Lu=class extends mu{constructor(e,t,n){n.enable=n.enable??!1,super(e,t,n),this.handleEvent=e=>{this.options.enable&&this.callback({type:`contextmenu`,center:{x:e.clientX,y:e.clientY},srcEvent:e,pointerType:`mouse`,target:e.target})},n.enable&&this.listen(`contextmenu`,!0)}destroy(){this.listen(`contextmenu`,!1)}enableEventType(e,t){e===`contextmenu`&&this.options.enable!==t&&(this.options.enable=t,this.listen(`contextmenu`,t))}},Ru=1,zu=2,Bu=4,Vu={pointerdown:Ru,pointermove:zu,pointerup:Bu,mousedown:Ru,mousemove:zu,mouseup:Bu},Hu=0,Uu=1,Wu=2,Gu=1,Ku=2,qu=4;function Ju(e){let t=Vu[e.srcEvent.type];if(!t)return null;let{buttons:n,button:r}=e.srcEvent,i=!1,a=!1,o=!1;return t===zu?(i=!!(n&Gu),a=!!(n&qu),o=!!(n&Ku)):(i=r===Hu,a=r===Uu,o=r===Wu),{leftButton:i,middleButton:a,rightButton:o}}function Yu(e,t){let n=e.center;if(!n)return null;let r=t.getBoundingClientRect(),i=r.width/t.offsetWidth||1,a=r.height/t.offsetHeight||1;return{center:n,offsetCenter:{x:(n.x-r.left-t.clientLeft)/i,y:(n.y-r.top-t.clientTop)/a}}}var Xu={srcElement:`root`,priority:0},Zu=class{constructor(e,t){this.handleEvent=e=>{if(this.isEmpty())return;let t=this._normalizeEvent(e),n=e.srcEvent.target;for(;n&&n!==t.rootElement;){if(this._emit(t,n),t.handled)return;n=n.parentNode}this._emit(t,`root`)},this.eventManager=e,this.recognizerName=t,this.handlers=[],this.handlersByElement=new Map,this._active=!1}isEmpty(){return!this._active}add(e,t,n,r=!1,i=!1){let{handlers:a,handlersByElement:o}=this,s={...Xu,...n},c=o.get(s.srcElement);c||(c=[],o.set(s.srcElement,c));let l={type:e,handler:t,srcElement:s.srcElement,priority:s.priority};r&&(l.once=!0),i&&(l.passive=!0),a.push(l),this._active=this._active||!l.passive;let u=c.length-1;for(;u>=0&&!(c[u].priority>=l.priority);)u--;c.splice(u+1,0,l)}remove(e,t){let{handlers:n,handlersByElement:r}=this;for(let i=n.length-1;i>=0;i--){let a=n[i];if(a.type===e&&a.handler===t){n.splice(i,1);let e=r.get(a.srcElement);e.splice(e.indexOf(a),1),e.length===0&&r.delete(a.srcElement)}}this._active=n.some(e=>!e.passive)}_emit(e,t){let n=this.handlersByElement.get(t);if(n){let t=!1,r=()=>{e.handled=!0},i=()=>{e.handled=!0,t=!0},a=[];for(let o=0;o<n.length;o++){let{type:s,handler:c,once:l}=n[o];if(c({...e,type:s,stopPropagation:r,stopImmediatePropagation:i}),l&&a.push(n[o]),t)break}for(let e=0;e<a.length;e++){let{type:t,handler:n}=a[e];this.remove(t,n)}}}_normalizeEvent(e){let t=this.eventManager.getElement();return{...e,...Ju(e),...Yu(e,t),preventDefault:()=>{e.srcEvent.preventDefault()},stopImmediatePropagation:null,stopPropagation:null,handled:!1,rootElement:t}}};function Qu(e){if(`recognizer`in e)return e;let t,n=Array.isArray(e)?[...e]:[e];return t=typeof n[0]==`function`?new(n.shift())(n.shift()||{}):n.shift(),{recognizer:t,recognizeWith:typeof n[0]==`string`?[n[0]]:n[0],requireFailure:typeof n[1]==`string`?[n[1]]:n[1]}}var $u=class{constructor(e=null,t={}){if(this._onBasicInput=e=>{this.manager.emit(e.srcEvent.type,e)},this._onOtherEvent=e=>{this.manager.emit(e.type,e)},this.options={recognizers:[],events:{},touchAction:`compute`,tabIndex:0,cssProps:{},...t},this.events=new Map,this.element=e,this.wheelSession=new Du(e),e){this.manager=new Ql(e,this.options);for(let e of this.options.recognizers){let{recognizer:t,recognizeWith:n,requireFailure:r}=Qu(e);this.manager.add(t),n&&t.recognizeWith(n),r&&t.requireFailure(r)}this.manager.on(`hammer.input`,this._onBasicInput),this.wheelInput=new vu(e,this._onOtherEvent,{enable:!1,wheelSession:this.wheelSession}),this.moveInput=new Pu(e,this._onOtherEvent,{enable:!1}),this.keyInput=new Iu(e,this._onOtherEvent,{enable:!1,tabIndex:t.tabIndex}),this.contextmenuInput=new Lu(e,this._onOtherEvent,{enable:!1}),this.on(this.options.events)}}getElement(){return this.element}destroy(){if(!this.element){this.wheelSession.destroy();return}this.wheelInput.destroy(),this.wheelSession.destroy(),this.moveInput.destroy(),this.keyInput.destroy(),this.contextmenuInput.destroy(),this.manager.destroy()}on(e,t,n){this._addEventHandler(e,t,n,!1)}once(e,t,n){this._addEventHandler(e,t,n,!0)}watch(e,t,n){this._addEventHandler(e,t,n,!1,!0)}off(e,t){this._removeEventHandler(e,t)}emit(e){this.manager?.emit(e.type,e)}_toggleRecognizer(e,t){let{manager:n}=this;if(!n)return;let r=n.get(e);r&&(r.set({enable:t,wheelSession:this.wheelSession}),n.touchAction.update()),this.wheelInput?.enableEventType(e,t),this.moveInput?.enableEventType(e,t),this.keyInput?.enableEventType(e,t),this.contextmenuInput?.enableEventType(e,t)}_addEventHandler(e,t,n,r,i){if(typeof e!=`string`){n=t;for(let[t,a]of Object.entries(e))this._addEventHandler(t,a,n,r,i);return}let{manager:a,events:o}=this;if(!a)return;let s=o.get(e);if(!s){let t=this._getRecognizerName(e)||e;s=new Zu(this,t),o.set(e,s),a&&a.on(e,s.handleEvent)}s.add(e,t,n,r,i),s.isEmpty()||this._toggleRecognizer(s.recognizerName,!0)}_removeEventHandler(e,t){if(typeof e!=`string`){for(let[t,n]of Object.entries(e))this._removeEventHandler(t,n);return}let{events:n}=this,r=n.get(e);if(r&&(r.remove(e,t),r.isEmpty())){let{recognizerName:e}=r,t=!1;for(let r of n.values())if(r.recognizerName===e&&!r.isEmpty()){t=!0;break}t||this._toggleRecognizer(e,!1)}}_getRecognizerName(e){return this.manager.recognizers.find(t=>t.getEventNames().includes(e))?.options.event}},ed={DEFAULT:`default`,LNGLAT:`lnglat`,METER_OFFSETS:`meter-offsets`,LNGLAT_OFFSETS:`lnglat-offsets`,CARTESIAN:`cartesian`};Object.defineProperty(ed,"IDENTITY",{get:()=>(z.deprecated(`COORDINATE_SYSTEM.IDENTITY`,`COORDINATE_SYSTEM.CARTESIAN`)(),ed.CARTESIAN)});var td={WEB_MERCATOR:1,GLOBE:2,WEB_MERCATOR_AUTO_OFFSET:4,IDENTITY:0},nd={common:0,meters:1,pixels:2},rd={click:`onClick`,dblclick:`onClick`,panstart:`onDragStart`,panmove:`onDrag`,panend:`onDragEnd`},id={multipan:[du,{threshold:10,pointers:2,trackpad:!0}],pinch:[pu,{trackpad:!0},null,[`multipan`]],pan:[du,{threshold:1},[`pinch`],[`multipan`]],dblclick:[cu,{event:`dblclick`,taps:2,enable:!1}],dblclickdrag:[su,{event:`dblclickdrag`,enable:!1},[`dblclick`],null],click:[cu,{event:`click`},[`dblclickdrag`],[`dblclick`,`dblclickdrag`]]};function ad(e,t){if(e===t)return!0;if(Array.isArray(e)){let n=e.length;if(!t||t.length!==n)return!1;for(let r=0;r<n;r++)if(e[r]!==t[r])return!1;return!0}return!1}function od(e){let t={},n;return r=>{for(let i in r)if(!ad(r[i],t[i])){n=e(r),t=r;break}return n}}var sd=[0,0,0,0],cd=[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,0],ld=[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],ud=[0,0,0],dd=[0,0,0],fd={default:-1,cartesian:0,lnglat:1,"meter-offsets":2,"lnglat-offsets":3};function pd(e){let t=fd[e];if(t===void 0)throw Error(`Invalid coordinateSystem: ${e}`);return t}var md=od(vd);function hd(e,t,n=dd){n.length<3&&(n=[n[0],n[1],0]);let r=n,i,a=!0;switch(i=t===`lnglat-offsets`||t===`meter-offsets`?n:e.isGeospatial?[Math.fround(e.longitude),Math.fround(e.latitude),0]:null,e.projectionMode){case td.WEB_MERCATOR:(t===`lnglat`||t===`cartesian`)&&(i=[0,0,0],a=!1);break;case td.WEB_MERCATOR_AUTO_OFFSET:t===`lnglat`?r=i:t===`cartesian`&&(r=[Math.fround(e.center[0]),Math.fround(e.center[1]),0],i=e.unprojectPosition(r),r[0]-=n[0],r[1]-=n[1],r[2]-=n[2]);break;case td.IDENTITY:r=e.position.map(Math.fround),r[2]=r[2]||0;break;case td.GLOBE:a=!1,i=null;break;default:a=!1}return{geospatialOrigin:i,shaderCoordinateOrigin:r,offsetMode:a}}function gd(e,t,n){let{viewMatrixUncentered:r,projectionMatrix:i}=e,{viewMatrix:a,viewProjectionMatrix:o}=e,s=sd,c=sd,l=e.cameraPosition,{geospatialOrigin:u,shaderCoordinateOrigin:d,offsetMode:f}=hd(e,t,n);return f&&(c=e.projectPosition(u||d),l=[l[0]-c[0],l[1]-c[1],l[2]-c[2]],c[3]=1,s=bn([],c,o),a=r||a,o=Xt([],i,a),o=Xt([],o,cd)),{viewMatrix:a,viewProjectionMatrix:o,projectionCenter:s,originCommon:c,cameraPosCommon:l,shaderCoordinateOrigin:d,geospatialOrigin:u}}function _d({viewport:e,devicePixelRatio:t=1,modelMatrix:n=null,coordinateSystem:r=`default`,coordinateOrigin:i=dd,autoWrapLongitude:a=!1}){r==="default"&&(r=e.isGeospatial?`lnglat`:`cartesian`);let o=md({viewport:e,devicePixelRatio:t,coordinateSystem:r,coordinateOrigin:i});return o.wrapLongitude=a,o.modelMatrix=n||ld,o}function vd({viewport:e,devicePixelRatio:t,coordinateSystem:n,coordinateOrigin:r}){let{projectionCenter:i,viewProjectionMatrix:a,originCommon:o,cameraPosCommon:s,shaderCoordinateOrigin:c,geospatialOrigin:l}=gd(e,n,r),u=e.getDistanceScales(),d=[e.width*t,e.height*t],f=bn([],[0,0,-e.focalDistance,1],e.projectionMatrix)[3]||1,p={coordinateSystem:pd(n),projectionMode:e.projectionMode,coordinateOrigin:c,commonOrigin:o.slice(0,3),center:i,pseudoMeters:!!e._pseudoMeters,viewportSize:d,devicePixelRatio:t,focalDistance:f,commonUnitsPerMeter:u.unitsPerMeter,commonUnitsPerWorldUnit:u.unitsPerMeter,commonUnitsPerWorldUnit2:ud,scale:e.scale,wrapLongitude:!1,viewProjectionMatrix:a,modelMatrix:ld,cameraPosition:s};if(l){let t=e.getDistanceScales(l);switch(n){case`meter-offsets`:p.commonUnitsPerWorldUnit=t.unitsPerMeter,p.commonUnitsPerWorldUnit2=t.unitsPerMeter2;break;case`lnglat`:case`lnglat-offsets`:e._pseudoMeters||(p.commonUnitsPerMeter=t.unitsPerMeter),p.commonUnitsPerWorldUnit=t.unitsPerDegree,p.commonUnitsPerWorldUnit2=t.unitsPerDegree2;break;case`cartesian`:p.commonUnitsPerWorldUnit=[1,1,t.unitsPerMeter[2]],p.commonUnitsPerWorldUnit2=[0,0,t.unitsPerMeter2[2]];break;default:break}}if(e.projectionMode===td.GLOBE&&n===`meter-offsets`){let e=r[0]*Math.PI/180,t=r[1]*Math.PI/180,n=Math.cos(t),i=((r[2]||0)/6370972+1)*256;p.commonOrigin=[Math.sin(e)*n*i,-Math.cos(e)*n*i,Math.sin(t)*i]}return p}var yd=`\
${`\
${[`default`,`lnglat`,`meter-offsets`,`lnglat-offsets`,`cartesian`].map(e=>`const COORDINATE_SYSTEM_${e.toUpperCase().replaceAll(`-`,`_`)}: i32 = ${pd(e)};`).join(``)}
${Object.keys(td).map(e=>`const PROJECTION_MODE_${e}: i32 = ${td[e]};`).join(``)}
${Object.keys(nd).map(e=>`const UNIT_${e.toUpperCase()}: i32 = ${nd[e]};`).join(``)}

const TILE_SIZE: f32 = 512.0;
const PI: f32 = 3.1415926536;
const WORLD_SCALE: f32 = TILE_SIZE / (PI * 2.0);
const ZERO_64_LOW: vec3<f32> = vec3<f32>(0.0, 0.0, 0.0);
const EARTH_RADIUS: f32 = 6370972.0; // meters
const GLOBE_RADIUS: f32 = 256.0;

// -----------------------------------------------------------------------------
// Uniform block (converted from GLSL uniform block)
// -----------------------------------------------------------------------------
struct ProjectUniforms {
  wrapLongitude: i32,
  coordinateSystem: i32,
  commonUnitsPerMeter: vec3<f32>,
  projectionMode: i32,
  scale: f32,
  commonUnitsPerWorldUnit: vec3<f32>,
  commonUnitsPerWorldUnit2: vec3<f32>,
  center: vec4<f32>,
  modelMatrix: mat4x4<f32>,
  viewProjectionMatrix: mat4x4<f32>,
  viewportSize: vec2<f32>,
  devicePixelRatio: f32,
  focalDistance: f32,
  cameraPosition: vec3<f32>,
  coordinateOrigin: vec3<f32>,
  commonOrigin: vec3<f32>,
  pseudoMeters: i32,
};

@group(0) @binding(auto)
var<uniform> project: ProjectUniforms;

// -----------------------------------------------------------------------------
// Geometry data shared across the project helpers.
// The active layer shader is responsible for populating this private module
// state before calling the project functions below.
// -----------------------------------------------------------------------------

// Structure to carry additional geometry data used by deck.gl filters.
struct Geometry {
  worldPosition: vec3<f32>,
  worldPositionAlt: vec3<f32>,
  position: vec4<f32>,
  normal: vec3<f32>,
  uv: vec2<f32>,
  pickingColor: vec3<f32>,
};

var<private> geometry: Geometry;
`}

// -----------------------------------------------------------------------------
// Functions
// -----------------------------------------------------------------------------

// Returns an adjustment factor for commonUnitsPerMeter
fn _project_size_at_latitude(lat: f32) -> f32 {
  let y = clamp(lat, -89.9, 89.9);
  return 1.0 / cos(radians(y));
}

// Overloaded version: scales a value in meters at a given latitude.
fn _project_size_at_latitude_m(meters: f32, lat: f32) -> f32 {
  return meters * project.commonUnitsPerMeter.z * _project_size_at_latitude(lat);
}

// Computes a non-linear scale factor based on geometry.
// (Note: This function relies on "geometry" being provided.)
fn project_size() -> f32 {
  if (project.projectionMode == PROJECTION_MODE_WEB_MERCATOR &&
      project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT &&
      project.pseudoMeters == 0) {
    if (geometry.position.w == 0.0) {
      return _project_size_at_latitude(geometry.worldPosition.y);
    }
    let y: f32 = geometry.position.y / TILE_SIZE * 2.0 - 1.0;
    let y2 = y * y;
    let y4 = y2 * y2;
    let y6 = y4 * y2;
    return 1.0 + 4.9348 * y2 + 4.0587 * y4 + 1.5642 * y6;
  }
  return 1.0;
}

// Overloads to scale offsets (meters to world units)
fn project_size_float(meters: f32) -> f32 {
  return meters * project.commonUnitsPerMeter.z * project_size();
}

fn project_size_vec2(meters: vec2<f32>) -> vec2<f32> {
  return meters * project.commonUnitsPerMeter.xy * project_size();
}

fn project_size_vec3(meters: vec3<f32>) -> vec3<f32> {
  return meters * project.commonUnitsPerMeter * project_size();
}

fn project_size_vec4(meters: vec4<f32>) -> vec4<f32> {
  return vec4<f32>(meters.xyz * project.commonUnitsPerMeter, meters.w);
}

// Returns a rotation matrix aligning the z‑axis with the given up vector.
fn project_get_orientation_matrix(up: vec3<f32>) -> mat3x3<f32> {
  let uz = normalize(up);
  let ux = select(
    vec3<f32>(1.0, 0.0, 0.0),
    normalize(vec3<f32>(uz.y, -uz.x, 0.0)),
    abs(uz.z) == 1.0
  );
  let uy = cross(uz, ux);
  return mat3x3<f32>(ux, uy, uz);
}

// Since WGSL does not support "out" parameters, we return a struct.
struct RotationResult {
  needsRotation: bool,
  transform: mat3x3<f32>,
};

fn project_needs_rotation(commonPosition: vec3<f32>) -> RotationResult {
  if (project.projectionMode == PROJECTION_MODE_GLOBE) {
    return RotationResult(true, project_get_orientation_matrix(commonPosition));
  } else {
    return RotationResult(false, mat3x3<f32>());  // identity alternative if needed
  };
}

// Projects a normal vector from the current coordinate system to world space.
fn project_normal(vector: vec3<f32>) -> vec3<f32> {
  let normal_modelspace = project.modelMatrix * vec4<f32>(vector, 0.0);
  var n = normalize(normal_modelspace.xyz * project.commonUnitsPerMeter);
  let rotResult = project_needs_rotation(geometry.position.xyz);
  if (rotResult.needsRotation) {
    n = rotResult.transform * n;
  }
  return n;
}

// Applies a scale offset based on y-offset (dy)
fn project_offset_(offset: vec4<f32>) -> vec4<f32> {
  let dy: f32 = offset.y;
  let commonUnitsPerWorldUnit = project.commonUnitsPerWorldUnit + project.commonUnitsPerWorldUnit2 * dy;
  return vec4<f32>(offset.xyz * commonUnitsPerWorldUnit, offset.w);
}

// Projects lng/lat coordinates to a unit tile [0,1]
fn project_mercator_(lnglat: vec2<f32>) -> vec2<f32> {
  var x = lnglat.x;
  if (project.wrapLongitude != 0) {
    x = ((x + 180.0) % 360.0) - 180.0;
  }
  let y = clamp(lnglat.y, -89.9, 89.9);
  return vec2<f32>(
    radians(x) + PI,
    PI + log(tan_fp32(PI * 0.25 + radians(y) * 0.5))
  ) * WORLD_SCALE;
}

// Projects lng/lat/z coordinates for a globe projection.
fn project_globe_(lnglatz: vec3<f32>) -> vec3<f32> {
  let lambda = radians(lnglatz.x);
  let phi = radians(lnglatz.y);
  let cosPhi = cos(phi);
  let D = (lnglatz.z / EARTH_RADIUS + 1.0) * GLOBE_RADIUS;
  return vec3<f32>(
    sin(lambda) * cosPhi,
    -cos(lambda) * cosPhi,
    sin(phi)
  ) * D;
}

// Projects positions (with an optional 64-bit low part) from the input
// coordinate system to the common space.
fn project_position_vec4_f64(position: vec4<f32>, position64Low: vec3<f32>) -> vec4<f32> {
  var position_world = project.modelMatrix * position;

  // Work around for a Mac+NVIDIA bug:
  if (project.projectionMode == PROJECTION_MODE_WEB_MERCATOR) {
    if (project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT) {
      return vec4<f32>(
        project_mercator_(position_world.xy),
        _project_size_at_latitude_m(position_world.z, position_world.y),
        position_world.w
      );
    }
    if (project.coordinateSystem == COORDINATE_SYSTEM_CARTESIAN) {
      position_world = vec4f(position_world.xyz + project.coordinateOrigin, position_world.w);
    }
  }
  if (project.projectionMode == PROJECTION_MODE_GLOBE) {
    if (project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT) {
      return vec4<f32>(
        project_globe_(position_world.xyz),
        position_world.w
      );
    }
    if (project.coordinateSystem == COORDINATE_SYSTEM_METER_OFFSETS) {
      let enuMatrix = project_get_orientation_matrix(project.commonOrigin);
      let metersToCommon = GLOBE_RADIUS / EARTH_RADIUS;
      let offsetCommon = (enuMatrix * vec3<f32>(-position_world.x, -position_world.y, position_world.z)) * metersToCommon;
      return vec4<f32>(project.commonOrigin + offsetCommon, position_world.w);
    }
  }
  if (project.projectionMode == PROJECTION_MODE_WEB_MERCATOR_AUTO_OFFSET) {
    if (project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT) {
      if (abs(position_world.y - project.coordinateOrigin.y) > 0.25) {
        return vec4<f32>(
          project_mercator_(position_world.xy) - project.commonOrigin.xy,
          project_size_float(position_world.z),
          position_world.w
        );
      }
    }
  }
  if (project.projectionMode == PROJECTION_MODE_IDENTITY ||
      (project.projectionMode == PROJECTION_MODE_WEB_MERCATOR_AUTO_OFFSET &&
       (project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT ||
        project.coordinateSystem == COORDINATE_SYSTEM_CARTESIAN))) {
    position_world = vec4f(position_world.xyz - project.coordinateOrigin, position_world.w);
  }

  return project_offset_(position_world) +
         project_offset_(project.modelMatrix * vec4<f32>(position64Low, 0.0));
}

// Overloaded versions for different input types.
fn project_position_vec4_f32(position: vec4<f32>) -> vec4<f32> {
  return project_position_vec4_f64(position, ZERO_64_LOW);
}

fn project_position_vec3_f64(position: vec3<f32>, position64Low: vec3<f32>) -> vec3<f32> {
  let projected_position = project_position_vec4_f64(vec4<f32>(position, 1.0), position64Low);
  return projected_position.xyz;
}

fn project_position_vec3_f32(position: vec3<f32>) -> vec3<f32> {
  let projected_position = project_position_vec4_f64(vec4<f32>(position, 1.0), ZERO_64_LOW);
  return projected_position.xyz;
}

fn project_position_vec2_f32(position: vec2<f32>) -> vec2<f32> {
  let projected_position = project_position_vec4_f64(vec4<f32>(position, 0.0, 1.0), ZERO_64_LOW);
  return projected_position.xy;
}

// Transforms a common space position to clip space.
fn project_common_position_to_clipspace_with_projection(position: vec4<f32>, viewProjectionMatrix: mat4x4<f32>, center: vec4<f32>) -> vec4<f32> {
  var clipPosition = viewProjectionMatrix * position + center;
  // deck.gl projection matrices use WebGL's [-w, w] depth range; WebGPU clips z to [0, w].
  clipPosition.z = (clipPosition.z + clipPosition.w) * 0.5;
  return clipPosition;
}

// Uses the project viewProjectionMatrix and center.
fn project_common_position_to_clipspace(position: vec4<f32>) -> vec4<f32> {
  return project_common_position_to_clipspace_with_projection(position, project.viewProjectionMatrix, project.center);
}

// Returns a clip space offset corresponding to a given number of screen pixels.
fn project_pixel_size_to_clipspace(pixels: vec2<f32>) -> vec2<f32> {
  let offset = pixels / project.viewportSize * project.devicePixelRatio * 2.0;
  return offset * project.focalDistance;
}

fn project_meter_size_to_pixel(meters: f32) -> f32 {
  return project_size_float(meters) * project.scale;
}

fn project_unit_size_to_pixel(size: f32, unit: i32) -> f32 {
  if (unit == UNIT_METERS) {
    return project_meter_size_to_pixel(size);
  } else if (unit == UNIT_COMMON) {
    return size * project.scale;
  }
  // UNIT_PIXELS: no scaling applied.
  return size;
}

fn project_pixel_size_float(pixels: f32) -> f32 {
  return pixels / project.scale;
}

fn project_pixel_size_vec2(pixels: vec2<f32>) -> vec2<f32> {
  return pixels / project.scale;
}
`,bd=`\
${[`default`,`lnglat`,`meter-offsets`,`lnglat-offsets`,`cartesian`].map(e=>`const int COORDINATE_SYSTEM_${e.toUpperCase().replaceAll(`-`,`_`)} = ${pd(e)};`).join(``)}
${Object.keys(td).map(e=>`const int PROJECTION_MODE_${e} = ${td[e]};`).join(``)}
${Object.keys(nd).map(e=>`const int UNIT_${e.toUpperCase()} = ${nd[e]};`).join(``)}
layout(std140) uniform projectUniforms {
bool wrapLongitude;
int coordinateSystem;
vec3 commonUnitsPerMeter;
int projectionMode;
float scale;
vec3 commonUnitsPerWorldUnit;
vec3 commonUnitsPerWorldUnit2;
vec4 center;
mat4 modelMatrix;
mat4 viewProjectionMatrix;
vec2 viewportSize;
float devicePixelRatio;
float focalDistance;
vec3 cameraPosition;
vec3 coordinateOrigin;
vec3 commonOrigin;
bool pseudoMeters;
} project;
const float TILE_SIZE = 512.0;
const float PI = 3.1415926536;
const float WORLD_SCALE = TILE_SIZE / (PI * 2.0);
const vec3 ZERO_64_LOW = vec3(0.0);
const float EARTH_RADIUS = 6370972.0;
const float GLOBE_RADIUS = 256.0;
float project_size_at_latitude(float lat) {
float y = clamp(lat, -89.9, 89.9);
return 1.0 / cos(radians(y));
}
float project_size() {
if (project.projectionMode == PROJECTION_MODE_WEB_MERCATOR &&
project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT &&
project.pseudoMeters == false) {
if (geometry.position.w == 0.0) {
return project_size_at_latitude(geometry.worldPosition.y);
}
float y = geometry.position.y / TILE_SIZE * 2.0 - 1.0;
float y2 = y * y;
float y4 = y2 * y2;
float y6 = y4 * y2;
return 1.0 + 4.9348 * y2 + 4.0587 * y4 + 1.5642 * y6;
}
return 1.0;
}
float project_size_at_latitude(float meters, float lat) {
return meters * project.commonUnitsPerMeter.z * project_size_at_latitude(lat);
}
float project_size(float meters) {
return meters * project.commonUnitsPerMeter.z * project_size();
}
vec2 project_size(vec2 meters) {
return meters * project.commonUnitsPerMeter.xy * project_size();
}
vec3 project_size(vec3 meters) {
return meters * project.commonUnitsPerMeter * project_size();
}
vec4 project_size(vec4 meters) {
return vec4(meters.xyz * project.commonUnitsPerMeter, meters.w);
}
mat3 project_get_orientation_matrix(vec3 up) {
vec3 uz = normalize(up);
vec3 ux = abs(uz.z) == 1.0 ? vec3(1.0, 0.0, 0.0) : normalize(vec3(uz.y, -uz.x, 0));
vec3 uy = cross(uz, ux);
return mat3(ux, uy, uz);
}
bool project_needs_rotation(vec3 commonPosition, out mat3 transform) {
if (project.projectionMode == PROJECTION_MODE_GLOBE) {
transform = project_get_orientation_matrix(commonPosition);
return true;
}
return false;
}
vec3 project_normal(vec3 vector) {
vec4 normal_modelspace = project.modelMatrix * vec4(vector, 0.0);
vec3 n = normalize(normal_modelspace.xyz * project.commonUnitsPerMeter);
mat3 rotation;
if (project_needs_rotation(geometry.position.xyz, rotation)) {
n = rotation * n;
}
return n;
}
vec4 project_offset_(vec4 offset) {
float dy = offset.y;
vec3 commonUnitsPerWorldUnit = project.commonUnitsPerWorldUnit + project.commonUnitsPerWorldUnit2 * dy;
return vec4(offset.xyz * commonUnitsPerWorldUnit, offset.w);
}
vec2 project_mercator_(vec2 lnglat) {
float x = lnglat.x;
if (project.wrapLongitude) {
x = mod(x + 180., 360.0) - 180.;
}
float y = clamp(lnglat.y, -89.9, 89.9);
return vec2(
radians(x) + PI,
PI + log(tan_fp32(PI * 0.25 + radians(y) * 0.5))
) * WORLD_SCALE;
}
vec3 project_globe_(vec3 lnglatz) {
float lambda = radians(lnglatz.x);
float phi = radians(lnglatz.y);
float cosPhi = cos(phi);
float D = (lnglatz.z / EARTH_RADIUS + 1.0) * GLOBE_RADIUS;
return vec3(
sin(lambda) * cosPhi,
-cos(lambda) * cosPhi,
sin(phi)
) * D;
}
vec4 project_position(vec4 position, vec3 position64Low) {
vec4 position_world = project.modelMatrix * position;
if (project.projectionMode == PROJECTION_MODE_WEB_MERCATOR) {
if (project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT) {
return vec4(
project_mercator_(position_world.xy),
project_size_at_latitude(position_world.z, position_world.y),
position_world.w
);
}
if (project.coordinateSystem == COORDINATE_SYSTEM_CARTESIAN) {
position_world.xyz += project.coordinateOrigin;
}
}
if (project.projectionMode == PROJECTION_MODE_GLOBE) {
if (project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT) {
return vec4(
project_globe_(position_world.xyz),
position_world.w
);
}
if (project.coordinateSystem == COORDINATE_SYSTEM_METER_OFFSETS) {
mat3 enuMatrix = project_get_orientation_matrix(project.commonOrigin);
float metersToCommon = GLOBE_RADIUS / EARTH_RADIUS;
vec3 offsetCommon = (enuMatrix * vec3(-position_world.xy, position_world.z)) * metersToCommon;
return vec4(project.commonOrigin + offsetCommon, position_world.w);
}
}
if (project.projectionMode == PROJECTION_MODE_WEB_MERCATOR_AUTO_OFFSET) {
if (project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT) {
if (abs(position_world.y - project.coordinateOrigin.y) > 0.25) {
return vec4(
project_mercator_(position_world.xy) - project.commonOrigin.xy,
project_size(position_world.z),
position_world.w
);
}
}
}
if (project.projectionMode == PROJECTION_MODE_IDENTITY ||
(project.projectionMode == PROJECTION_MODE_WEB_MERCATOR_AUTO_OFFSET &&
(project.coordinateSystem == COORDINATE_SYSTEM_LNGLAT ||
project.coordinateSystem == COORDINATE_SYSTEM_CARTESIAN))) {
position_world.xyz -= project.coordinateOrigin;
}
return project_offset_(position_world) + project_offset_(project.modelMatrix * vec4(position64Low, 0.0));
}
vec4 project_position(vec4 position) {
return project_position(position, ZERO_64_LOW);
}
vec3 project_position(vec3 position, vec3 position64Low) {
vec4 projected_position = project_position(vec4(position, 1.0), position64Low);
return projected_position.xyz;
}
vec3 project_position(vec3 position) {
vec4 projected_position = project_position(vec4(position, 1.0), ZERO_64_LOW);
return projected_position.xyz;
}
vec2 project_position(vec2 position) {
vec4 projected_position = project_position(vec4(position, 0.0, 1.0), ZERO_64_LOW);
return projected_position.xy;
}
vec4 project_common_position_to_clipspace(vec4 position, mat4 viewProjectionMatrix, vec4 center) {
return viewProjectionMatrix * position + center;
}
vec4 project_common_position_to_clipspace(vec4 position) {
return project_common_position_to_clipspace(position, project.viewProjectionMatrix, project.center);
}
vec2 project_pixel_size_to_clipspace(vec2 pixels) {
vec2 offset = pixels / project.viewportSize * project.devicePixelRatio * 2.0;
return offset * project.focalDistance;
}
float project_size_to_pixel(float meters) {
return project_size(meters) * project.scale;
}
vec2 project_size_to_pixel(vec2 meters) {
return project_size(meters) * project.scale;
}
float project_size_to_pixel(float size, int unit) {
if (unit == UNIT_METERS) return project_size_to_pixel(size);
if (unit == UNIT_COMMON) return size * project.scale;
return size;
}
float project_pixel_size(float pixels) {
return pixels / project.scale;
}
vec2 project_pixel_size(vec2 pixels) {
return pixels / project.scale;
}
`,xd={};function Sd(e=xd){return`viewport`in e?_d(e):{}}var Cd={name:`project`,dependencies:[x,dl],source:yd,vs:bd,getUniforms:Sd,uniformTypes:{wrapLongitude:`f32`,coordinateSystem:`i32`,commonUnitsPerMeter:`vec3<f32>`,projectionMode:`i32`,scale:`f32`,commonUnitsPerWorldUnit:`vec3<f32>`,commonUnitsPerWorldUnit2:`vec3<f32>`,center:`vec4<f32>`,modelMatrix:`mat4x4<f32>`,viewProjectionMatrix:`mat4x4<f32>`,viewportSize:`vec2<f32>`,devicePixelRatio:`f32`,focalDistance:`f32`,cameraPosition:`vec3<f32>`,coordinateOrigin:`vec3<f32>`,commonOrigin:`vec3<f32>`,pseudoMeters:`f32`}},wd={name:`project32`,dependencies:[Cd],source:`// Define a structure to hold both the clip-space position and the common position.
struct ProjectResult {
  clipPosition: vec4<f32>,
  commonPosition: vec4<f32>,
};

// This function mimics the GLSL version with the 'out' parameter by returning both values.
fn project_position_to_clipspace_and_commonspace(
    position: vec3<f32>,
    position64Low: vec3<f32>,
    offset: vec3<f32>
) -> ProjectResult {
  // Compute the projected position.
  let projectedPosition: vec3<f32> = project_position_vec3_f64(position, position64Low);

  // Start with the provided offset.
  var finalOffset: vec3<f32> = offset;

  // Get whether a rotation is needed and the rotation matrix.
  let rotationResult = project_needs_rotation(projectedPosition);

  // If rotation is needed, update the offset.
  if (rotationResult.needsRotation) {
    finalOffset = rotationResult.transform * offset;
  }

  // Compute the common position.
  let commonPosition: vec4<f32> = vec4<f32>(projectedPosition + finalOffset, 1.0);

  // Convert to clip-space.
  let clipPosition: vec4<f32> = project_common_position_to_clipspace(commonPosition);

  return ProjectResult(clipPosition, commonPosition);
}

// A convenience overload that returns only the clip-space position.
fn project_position_to_clipspace(
    position: vec3<f32>,
    position64Low: vec3<f32>,
    offset: vec3<f32>
) -> vec4<f32> {
  return project_position_to_clipspace_and_commonspace(position, position64Low, offset).clipPosition;
}
`,vs:`vec4 project_position_to_clipspace(
  vec3 position, vec3 position64Low, vec3 offset, out vec4 commonPosition
) {
  vec3 projectedPosition = project_position(position, position64Low);
  mat3 rotation;
  if (project_needs_rotation(projectedPosition, rotation)) {
    // offset is specified as ENU
    // when in globe projection, rotate offset so that the ground alighs with the surface of the globe
    offset = rotation * offset;
  }
  commonPosition = vec4(projectedPosition + offset, 1.0);
  return project_common_position_to_clipspace(commonPosition);
}

vec4 project_position_to_clipspace(
  vec3 position, vec3 position64Low, vec3 offset
) {
  vec4 commonPosition;
  return project_position_to_clipspace(position, position64Low, offset, commonPosition);
}
`};function Td(){return[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}function Ed(e,t){let n=bn([],t,e);return mn(n,n,1/n[3]),n}function Dd(e,t,n){return e<t?t:e>n?n:e}function Od(e){return Math.log(e)*Math.LOG2E}var kd=Math.log2||Od;function Ad(e,t){if(!e)throw Error(t||`@math.gl/web-mercator: assertion failed.`)}var jd=Math.PI,Md=jd/4,Nd=jd/180,Pd=180/jd,Fd=512,Id=4003e4,Ld=85.051129,Rd=1.5;function zd(e){return kd(e)}function Bd(e){let[t,n]=e;Ad(Number.isFinite(t)),Ad(Number.isFinite(n)&&n>=-90&&n<=90,`invalid latitude`);let r=t*Nd,i=n*Nd;return[Fd*(r+jd)/(2*jd),Fd*(jd+Math.log(Math.tan(Md+i*.5)))/(2*jd)]}function Vd(e){let[t,n]=e,r=t/Fd*(2*jd)-jd,i=2*(Math.atan(Math.exp(n/Fd*(2*jd)-jd))-Md);return[r*Pd,i*Pd]}function Hd(e){let{latitude:t}=e;return Ad(Number.isFinite(t)),zd(Id*Math.cos(t*Nd))-9}function Ud(e){let t=Math.cos(e*Nd);return Fd/Id/t}function Wd(e){let{latitude:t,longitude:n,highPrecision:r=!1}=e;Ad(Number.isFinite(t)&&Number.isFinite(n));let i=Fd,a=Math.cos(t*Nd),o=i/360,s=o/a,c=i/Id/a,l={unitsPerMeter:[c,c,c],metersPerUnit:[1/c,1/c,1/c],unitsPerDegree:[o,s,c],degreesPerUnit:[1/o,1/s,1/c]};if(r){let e=Nd*Math.tan(t*Nd)/a,n=o*e/2,r=i/Id*e,u=r/s*c;l.unitsPerDegree2=[0,n,r],l.unitsPerMeter2=[u,0,u]}return l}function Gd(e,t){let[n,r,i]=e,[a,o,s]=t,{unitsPerMeter:c,unitsPerMeter2:l}=Wd({longitude:n,latitude:r,highPrecision:!0}),u=Bd(e);u[0]+=a*(c[0]+l[0]*o),u[1]+=o*(c[1]+l[1]*o);let d=Vd(u),f=(i||0)+(s||0);return Number.isFinite(i)||Number.isFinite(s)?[d[0],d[1],f]:d}function Kd(e){let{height:t,pitch:n,bearing:r,altitude:i,scale:a,center:o}=e,s=Td();Zt(s,s,[0,0,-i]),en(s,s,-n*Nd),nn(s,s,r*Nd);let c=a/t;return Qt(s,s,[c,c,c]),o&&Zt(s,s,dt([],o)),s}function qd(e){let{width:t,height:n,altitude:r,pitch:i=0,offset:a,center:o,scale:s,nearZMultiplier:c=1,farZMultiplier:l=1}=e,{fovy:u=Jd(Rd)}=e;r!==void 0&&(u=Jd(r));let d=u*Nd,f=i*Nd,p=Yd(u),m=p;o&&(m+=o[2]*s/Math.cos(f)/n);let h=d*(.5+(a?a[1]:0)/n),g=Math.sin(h)*m/Math.sin(Dd(Math.PI/2-f-h,.01,Math.PI-.01)),_=Math.sin(f)*g+m,v=m*10,y=Math.min(_*l,v);return{fov:d,aspect:t/n,focalDistance:p,near:c,far:y}}function Jd(e){return 2*Math.atan(.5/e)*Pd}function Yd(e){return .5/Math.tan(.5*e*Nd)}function Xd(e,t){let[n,r,i=0]=e;return Ad(Number.isFinite(n)&&Number.isFinite(r)&&Number.isFinite(i)),Ed(t,[n,r,i,1])}function Zd(e,t,n=0){let[r,i,a]=e;if(Ad(Number.isFinite(r)&&Number.isFinite(i),`invalid pixel coordinate`),Number.isFinite(a))return Ed(t,[r,i,a,1]);let o=Ed(t,[r,i,0,1]),s=Ed(t,[r,i,1,1]),c=o[2],l=s[2];return Je([],o,s,c===l?0:((n||0)-c)/(l-c))}function Qd(e){let{width:t,height:n,bounds:r,minExtent:i=0,maxZoom:a=24,offset:o=[0,0]}=e,[[s,c],[l,u]]=r,d=$d(e.padding),f=Bd([s,Dd(u,-Ld,Ld)]),p=Bd([l,Dd(c,-Ld,Ld)]),m=[Math.max(Math.abs(p[0]-f[0]),i),Math.max(Math.abs(p[1]-f[1]),i)],h=[t-d.left-d.right-Math.abs(o[0])*2,n-d.top-d.bottom-Math.abs(o[1])*2];Ad(h[0]>0&&h[1]>0);let g=h[0]/m[0],_=h[1]/m[1],v=(d.right-d.left)/2/g,y=(d.top-d.bottom)/2/_,b=Vd([(p[0]+f[0])/2+v,(p[1]+f[1])/2+y]),x=Math.min(a,kd(Math.abs(Math.min(g,_))));return Ad(Number.isFinite(x)),{longitude:b[0],latitude:b[1],zoom:x}}function $d(e=0){return typeof e==`number`?{top:e,bottom:e,left:e,right:e}:(Ad(Number.isFinite(e.top)&&Number.isFinite(e.bottom)&&Number.isFinite(e.left)&&Number.isFinite(e.right)),e)}var ef=Math.PI/180;function tf(e,t=0){let{width:n,height:r,unproject:i}=e,a={targetZ:t},o=i([0,r],a),s=i([n,r],a),c,l;return(e.fovy?.5*e.fovy*ef:Math.atan(.5/e.altitude))>(90-e.pitch)*ef-.01?(c=nf(e,0,t),l=nf(e,n,t)):(c=i([0,0],a),l=i([n,0],a)),[o,s,l,c]}function nf(e,t,n){let{pixelUnprojectionMatrix:r}=e,i=Ed(r,[t,0,1,1]),a=Ed(r,[t,e.height,1,1]),o=Vd(Je([],i,a,(n*e.distanceScales.unitsPerMeter[2]-i[2])/(a[2]-i[2])));return o.push(n),o}var rf=`
layout(std140) uniform shadowUniforms {
  bool drawShadowMap;
  bool useShadowMap;
  vec4 color;
  highp int lightId;
  float lightCount;
  mat4 viewProjectionMatrix0;
  mat4 viewProjectionMatrix1;
  vec4 projectCenter0;
  vec4 projectCenter1;
} shadow;
`,af=`
${rf}

const int max_lights = 2;

out vec3 shadow_vPosition[max_lights];

vec4 shadow_setVertexPosition(vec4 position_commonspace) {
  mat4 viewProjectionMatrices[max_lights];
  viewProjectionMatrices[0] = shadow.viewProjectionMatrix0;
  viewProjectionMatrices[1] = shadow.viewProjectionMatrix1;
  vec4 projectCenters[max_lights];
  projectCenters[0] = shadow.projectCenter0;
  projectCenters[1] = shadow.projectCenter1;

  if (shadow.drawShadowMap) {
    return project_common_position_to_clipspace(position_commonspace, viewProjectionMatrices[shadow.lightId], projectCenters[shadow.lightId]);
  }
  if (shadow.useShadowMap) {
    for (int i = 0; i < max_lights; i++) {
      if(i < int(shadow.lightCount)) {
        vec4 shadowMap_position = project_common_position_to_clipspace(position_commonspace, viewProjectionMatrices[i], projectCenters[i]);
        shadow_vPosition[i] = (shadowMap_position.xyz / shadowMap_position.w + 1.0) / 2.0;
      }
    }
  }
  return gl_Position;
}

`,of=`
${rf}

const int max_lights = 2;
uniform sampler2D shadow_uShadowMap0;
uniform sampler2D shadow_uShadowMap1;

in vec3 shadow_vPosition[max_lights];

const vec4 bitPackShift = vec4(1.0, 255.0, 65025.0, 16581375.0);
const vec4 bitUnpackShift = 1.0 / bitPackShift;
const vec4 bitMask = vec4(1.0 / 255.0, 1.0 / 255.0, 1.0 / 255.0,  0.0);

float shadow_getShadowWeight(vec3 position, sampler2D shadowMap) {
  vec4 rgbaDepth = texture(shadowMap, position.xy);

  float z = dot(rgbaDepth, bitUnpackShift);
  return smoothstep(0.001, 0.01, position.z - z);
}

vec4 shadow_filterShadowColor(vec4 color) {
  if (shadow.drawShadowMap) {
    vec4 rgbaDepth = fract(gl_FragCoord.z * bitPackShift);
    rgbaDepth -= rgbaDepth.gbaa * bitMask;
    return rgbaDepth;
  }
  if (shadow.useShadowMap) {
    float shadowAlpha = 0.0;
    shadowAlpha += shadow_getShadowWeight(shadow_vPosition[0], shadow_uShadowMap0);
    if(shadow.lightCount > 1.0) {
      shadowAlpha += shadow_getShadowWeight(shadow_vPosition[1], shadow_uShadowMap1);
    }
    shadowAlpha *= shadow.color.a / shadow.lightCount;
    float blendedAlpha = shadowAlpha + color.a * (1.0 - shadowAlpha);

    return vec4(
      mix(color.rgb, shadow.color.rgb, shadowAlpha / blendedAlpha),
      blendedAlpha
    );
  }
  return color;
}

`,sf=od(ff),cf=od(pf),lf=[0,0,0,1],uf=[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,0];function df(e,t){let[n,r,i]=e,a=Zd([n,r,i],t);return Number.isFinite(i)?a:[a[0],a[1],0]}function ff({viewport:e,center:t}){return new R(e.viewProjectionMatrix).invert().transform(t)}function pf({viewport:e,shadowMatrices:t}){let n=[],r=e.pixelUnprojectionMatrix,i=e.isGeospatial?void 0:1,a=[[0,0,i],[e.width,0,i],[0,e.height,i],[e.width,e.height,i],[0,0,-1],[e.width,0,-1],[0,e.height,-1],[e.width,e.height,-1]].map(e=>df(e,r));for(let r of t){let t=r.clone().translate(new I(e.center).negate()),i=a.map(e=>t.transform(e)),o=new R().ortho({left:Math.min(...i.map(e=>e[0])),right:Math.max(...i.map(e=>e[0])),bottom:Math.min(...i.map(e=>e[1])),top:Math.max(...i.map(e=>e[1])),near:Math.min(...i.map(e=>-e[2])),far:Math.max(...i.map(e=>-e[2]))});n.push(o.multiplyRight(r))}return n}function mf(e){let{shadowEnabled:t=!0,project:n}=e;if(!t||!n||!e.shadowMatrices||!e.shadowMatrices.length)return{drawShadowMap:!1,useShadowMap:!1,shadow_uShadowMap0:e.dummyShadowMap,shadow_uShadowMap1:e.dummyShadowMap};let r=Cd.getUniforms(n),i=sf({viewport:n.viewport,center:r.center}),a=[],o=cf({shadowMatrices:e.shadowMatrices,viewport:n.viewport}).slice();for(let t=0;t<e.shadowMatrices.length;t++){let e=o[t],s=e.clone().translate(new I(n.viewport.center).negate());r.coordinateSystem===pd(`lnglat`)&&r.projectionMode===td.WEB_MERCATOR?(o[t]=s,a[t]=i):(o[t]=e.clone().multiplyRight(uf),a[t]=s.transform(i))}let s={drawShadowMap:!!e.drawToShadowMap,useShadowMap:e.shadowMaps?e.shadowMaps.length>0:!1,color:e.shadowColor||lf,lightId:e.shadowLightId||0,lightCount:e.shadowMatrices.length,shadow_uShadowMap0:e.dummyShadowMap,shadow_uShadowMap1:e.dummyShadowMap};for(let e=0;e<o.length;e++)s[`viewProjectionMatrix${e}`]=o[e],s[`projectCenter${e}`]=a[e];for(let t=0;t<2;t++)s[`shadow_uShadowMap${t}`]=e.shadowMaps&&e.shadowMaps[t]||e.dummyShadowMap;return s}var hf={name:`shadow`,dependencies:[Cd],vs:af,fs:of,inject:{"vs:DECKGL_FILTER_GL_POSITION":`
    position = shadow_setVertexPosition(geometry.position);
    `,"fs:DECKGL_FILTER_COLOR":`
    color = shadow_filterShadowColor(color);
    `},getUniforms:mf,uniformTypes:{drawShadowMap:`f32`,useShadowMap:`f32`,color:`vec4<f32>`,lightId:`i32`,lightCount:`f32`,viewProjectionMatrix0:`mat4x4<f32>`,viewProjectionMatrix1:`mat4x4<f32>`,projectCenter0:`vec4<f32>`,projectCenter1:`vec4<f32>`}},gf=16777215;function _f(e,t){e.length===10?z.warn(`pickMultipleObjects can only exclude 10 previously picked objects for layers without picking buffers`)():e.push(t)}var vf=`  float disabledPickingIndexCount;
  vec4 disabledPickingIndices0;
  vec4 disabledPickingIndices1;
  vec4 disabledPickingIndices2;
`;function yf(e){return e.replace(`  vec4 highlightColor;
} picking;`,`  vec4 highlightColor;\n${vf}} picking;`)}function bf(e,t){return[e[t]||0,e[t+1]||0,e[t+2]||0,e[t+3]||0]}var xf=`\
vec3 picking_getPickingColorFromIndex(float objectIndex) {
  if (objectIndex < 0.0 || objectIndex >= ${gf}.0) {
    return vec3(0.0);
  }

  for (int i = 0; i < 10; i++) {
    if (float(i) >= picking.disabledPickingIndexCount) {
      break;
    }
    vec4 disabledIndices = i < 4
      ? picking.disabledPickingIndices0
      : (i < 8 ? picking.disabledPickingIndices1 : picking.disabledPickingIndices2);
    float disabledIndex = disabledIndices[i - (i / 4) * 4];
    if (disabledIndex == objectIndex) {
      return vec3(0.0);
    }
  }

  float encodedIndex = objectIndex + 1.0;
  return vec3(
    mod(encodedIndex, 256.0),
    mod(floor(encodedIndex / 256.0), 256.0),
    mod(floor(encodedIndex / 65536.0), 256.0)
  );
}

vec3 picking_getPickingColorFromIndex(uint objectIndex) {
  return picking_getPickingColorFromIndex(float(objectIndex));
}

vec3 picking_getPickingColorFromInstanceID() {
  return picking_getPickingColorFromIndex(float(gl_InstanceID));
}

void picking_setPickingColorFromInstanceID() {
  picking_setPickingColor(picking_getPickingColorFromInstanceID());
}
`,Sf=`\
struct pickingUniforms {
  isActive: f32,
  isAttribute: f32,
  isHighlightActive: f32,
  useByteColors: f32,
  highlightedObjectColor: vec3<f32>,
  highlightColor: vec4<f32>,
  disabledPickingIndexCount: f32,
  disabledPickingIndices0: vec4<f32>,
  disabledPickingIndices1: vec4<f32>,
  disabledPickingIndices2: vec4<f32>,
};

@group(0) @binding(auto) var<uniform> picking: pickingUniforms;

fn picking_normalizeColor(color: vec3<f32>) -> vec3<f32> {
  return select(color, color / 255.0, picking.useByteColors > 0.5);
}

fn picking_normalizeColor4(color: vec4<f32>) -> vec4<f32> {
  return select(color, color / 255.0, picking.useByteColors > 0.5);
}

fn picking_isColorZero(color: vec3<f32>) -> bool {
  return dot(color, vec3<f32>(1.0)) < 0.00001;
}

fn picking_isColorValid(color: vec3<f32>) -> bool {
  return dot(color, vec3<f32>(1.0)) > 0.00001;
}

fn picking_getPickingColorFromIndex(objectIndex: u32) -> vec3<f32> {
  if (objectIndex >= ${gf}u) {
    return vec3<f32>(0.0);
  }

  for (var i = 0; i < 10; i = i + 1) {
    if (f32(i) >= picking.disabledPickingIndexCount) {
      break;
    }
    let disabledIndices = select(
      picking.disabledPickingIndices2,
      select(picking.disabledPickingIndices1, picking.disabledPickingIndices0, i < 4),
      i < 8
    );
    let disabledIndex = disabledIndices[i % 4];
    if (disabledIndex == f32(objectIndex)) {
      return vec3<f32>(0.0);
    }
  }

  let encodedIndex = objectIndex + 1u;
  return vec3<f32>(
    f32(encodedIndex % 256u),
    f32((encodedIndex / 256u) % 256u),
    f32((encodedIndex / 65536u) % 256u)
  ) / 255.0;
}
`,Cf={...Cr,vs:`${yf(Cr.vs)}\n${xf}`,fs:yf(Cr.fs),source:Sf,uniformTypes:{...Cr.uniformTypes,disabledPickingIndexCount:`f32`,disabledPickingIndices0:`vec4<f32>`,disabledPickingIndices1:`vec4<f32>`,disabledPickingIndices2:`vec4<f32>`},defaultUniforms:{...Cr.defaultUniforms,useByteColors:!0,disabledPickingIndexCount:0,disabledPickingIndices0:[0,0,0,0],disabledPickingIndices1:[0,0,0,0],disabledPickingIndices2:[0,0,0,0]},getUniforms(e,t){let n=Cr.getUniforms(e,t),r=e.disabledPickingIndices||[];return n.disabledPickingIndexCount=r.length,n.disabledPickingIndices0=bf(r,0),n.disabledPickingIndices1=bf(r,4),n.disabledPickingIndices2=bf(r,8),n},inject:{"vs:DECKGL_FILTER_GL_POSITION":`
    // for picking depth values
    picking_setPickingAttribute(position.z / position.w);
  `,"vs:DECKGL_FILTER_COLOR":`
  picking_setPickingColor(geometry.pickingColor);
  `,"fs:DECKGL_FILTER_COLOR":{order:99,injection:`
  // use highlight color if this fragment belongs to the selected object.
  color = picking_filterHighlightColor(color);

  // use picking color if rendering to picking FBO.
  color = picking_filterPickingColor(color);
    `}}},wf=[dl],Tf=[`vs:DECKGL_FILTER_SIZE(inout vec3 size, VertexGeometry geometry)`,`vs:DECKGL_FILTER_GL_POSITION(inout vec4 position, VertexGeometry geometry)`,`vs:DECKGL_FILTER_COLOR(inout vec4 color, VertexGeometry geometry)`,`fs:DECKGL_FILTER_COLOR(inout vec4 color, FragmentGeometry geometry)`],Ef=[];function Df(e){let t=b.getDefaultShaderAssembler(e);for(let e of wf)t.addDefaultModule(e);t._hookFunctions.length=0;let n=e===`glsl`?Tf:Ef;for(let e of n)t.addShaderHook(e);return t}var Of=[255,255,255],kf=1,Af=0,jf=class{constructor(e={}){this.type=`ambient`;let{color:t=Of}=e,{intensity:n=kf}=e;this.id=e.id||`ambient-${Af++}`,this.color=t,this.intensity=n}},Mf=[255,255,255],Nf=1,Pf=[0,0,-1],Ff=0,If=class{constructor(e={}){this.type=`directional`;let{color:t=Mf}=e,{intensity:n=Nf}=e,{direction:r=Pf}=e,{_shadow:i=!1}=e;this.id=e.id||`directional-${Ff++}`,this.color=t,this.intensity=n,this.type=`directional`,this.direction=new I(r).normalize().toArray(),this.shadow=i}getProjectedLight(e){return this}},Lf=class{constructor(e,t={id:`pass`}){let{id:n}=t;this.id=n,this.device=e,this.props={...t}}setProps(e){Object.assign(this.props,e)}render(e){}cleanup(){}},Rf={depthWriteEnabled:!0,depthCompare:`less-equal`,blendColorOperation:`add`,blendColorSrcFactor:`one`,blendColorDstFactor:`one-minus-src-alpha`,blendAlphaOperation:`add`,blendAlphaSrcFactor:`one`,blendAlphaDstFactor:`one-minus-src-alpha`},zf=class extends Lf{constructor(){super(...arguments),this._lastRenderIndex=-1}render(e){this._render(e)}_render(e){let{canvasContext:t=this.device.canvasContext}=e,n=e.target??t.getCurrentFramebuffer(),[r,i]=t.getDrawingBufferSize(),a=e.clearCanvas??!0,o=e.clearColor??(a?[0,0,0,0]:!1),s=a?1:!1,c=a?0:!1,l=e.colorMask??15,u={viewport:[0,0,r,i]};e.colorMask&&(u.colorMask=l),e.scissorRect&&(u.scissorRect=e.scissorRect);let{shaderModuleProps:d,viewports:f,views:p,onViewportActive:m,clearStack:h=!0}=e,g=e.pass||`unknown`,_=this.device.type===`webgpu`;h&&(this._lastRenderIndex=-1);let v=[];if(!f.length)return this.device.beginRenderPass({framebuffer:n,parameters:u,clearColor:o,clearDepth:s,clearStencil:c}).end(),this.device.submit(),v;try{for(let r of f){m?.(r);let i=this._getDrawLayerParams(r,e),a=p&&p[r.id],l=r.subViewports||[r],f=_?l.map(e=>[e]):[l];for(let r of f){let l=this.device.beginRenderPass({framebuffer:n,parameters:u,clearColor:o,clearDepth:s,clearStencil:c});try{for(let o of r){let r=this._drawLayersInViewport(l,{target:n,canvasContext:t,shaderModuleProps:d,viewport:o,view:a,pass:g,layers:e.layers,isPicking:e.isPicking},i);v.push(r)}}finally{l.end(),_&&this.device.submit()}o=!1,s=!1,c=!1}}return v}finally{_||this.device.submit()}}_getDrawLayerParams(e,{layers:t,pass:n,isPicking:r=!1,layerFilter:i,cullRect:a,views:o,effects:s,canvasContext:c=this.device.canvasContext,shaderModuleProps:l},u=!1){let d=[],f=Bf(this._lastRenderIndex+1),p={layer:t[0],viewport:e,isPicking:r,renderPass:n,cullRect:a},m={};for(let r=0;r<t.length;r++){let a=t[r],h=this._shouldDrawLayer(a,p,i,m),g={shouldDrawLayer:h};h&&!u&&(g.shouldDrawLayer=!0,g.layerRenderIndex=f(a,h),g.shaderModuleProps=this._getShaderModuleProps(a,s,n,c,l),g.layerParameters={...a.context.device.type===`webgpu`?Rf:null,...a.context.deck?.props.parameters,...o?.[e.id]?.props.parameters,...this.getLayerParameters(a,r,e)}),d[r]=g}return d}_drawLayersInViewport(e,{layers:t,shaderModuleProps:n,pass:r,target:i,canvasContext:a,viewport:o,view:s,isPicking:c},l){let u=Vf(this.device,{canvasContext:a,shaderModuleProps:n,target:i,viewport:o});if(s){let{clear:e,clearColor:t,clearDepth:n,clearStencil:r}=s.props;if(e){let e=[0,0,0,0],a=1,o=0;Array.isArray(t)&&!c?e=[...t.slice(0,3),t[3]||255].map(e=>e/255):t===!1&&(e=!1),n!==void 0&&(a=n),r!==void 0&&(o=r),this.device.beginRenderPass({framebuffer:i,parameters:{viewport:u,scissorRect:u},clearColor:e,clearDepth:a,clearStencil:o}).end()}}let d={totalCount:t.length,visibleCount:0,compositeCount:0,pickableCount:0};e.setParameters({viewport:u});for(let n=0;n<t.length;n++){let i=t[n],a=l[n],{shouldDrawLayer:s}=a;if(s&&i.props.pickable&&d.pickableCount++,i.isComposite&&d.compositeCount++,i.isDrawable&&a.shouldDrawLayer){let{layerRenderIndex:t,shaderModuleProps:n,layerParameters:s}=a;d.visibleCount++,this._lastRenderIndex=Math.max(this._lastRenderIndex,t),n.project&&(n.project.viewport=o),i.context.renderPass=e;try{i._drawLayer({renderPass:e,shaderModuleProps:n,uniforms:{layerIndex:t},parameters:s})}catch(e){i.raiseError(e,`drawing ${i} to ${r}`)}}}return d}shouldDrawLayer(e){return!0}getShaderModuleProps(e,t,n){return null}getLayerParameters(e,t,n){return e.props.parameters}_shouldDrawLayer(e,t,n,r){if(!(e.props.visible&&this.shouldDrawLayer(e)))return!1;t.layer=e;let i=e.parent;for(;i;){if(!i.props.visible||!i.filterSubLayer(t))return!1;t.layer=i,i=i.parent}if(n){let e=t.layer.id;if(e in r||(r[e]=n(t)),!r[e])return!1}return e.activateViewport(t.viewport),!0}_getShaderModuleProps(e,t,n,r,i){let a=r.cssToDeviceRatio(),o=e.internalState?.propsInTransition||e.props,s={layer:o,picking:{isActive:!1},project:{viewport:e.context.viewport,devicePixelRatio:a,modelMatrix:o.modelMatrix,coordinateSystem:o.coordinateSystem,coordinateOrigin:o.coordinateOrigin,autoWrapLongitude:e.wrapLongitude}};if(t)for(let n of t)Hf(s,n.getShaderModuleProps?.(e,s));for(let t of e.context.defaultShaderModules)t.name in s||(s[t.name]={});return Hf(s,this.getShaderModuleProps(e,t,s),i)}};function Bf(e=0,t={}){let n={},r=(i,a)=>{let o=i.props._offset,s=i.id,c=i.parent&&i.parent.id,l;if(c&&!(c in t)&&r(i.parent,!1),c in n){let e=n[c]=n[c]||Bf(t[c],t);l=e(i,a),n[s]=e}else Number.isFinite(o)?(l=o+(t[c]||0),n[s]=null):l=e;return a&&l>=e&&(e=l+1),t[s]=l,l};return r}function Vf(e,{canvasContext:t=e.canvasContext,shaderModuleProps:n,target:r,viewport:i}){let a=n?.project?.devicePixelRatio??t.cssToDeviceRatio(),[,o]=t.getDrawingBufferSize(),s=r?r.height:o,c=i;return[c.x*a,s-(c.y+c.height)*a,c.width*a,c.height*a]}function Hf(e,...t){for(let n of t)if(n)for(let t in n)e[t]?Object.assign(e[t],n[t]):e[t]=n[t];return e}var Uf=class extends zf{constructor(e,t){super(e,t);let n=e.createTexture({format:`rgba8unorm`,width:1,height:1,sampler:{minFilter:`linear`,magFilter:`linear`,addressModeU:`clamp-to-edge`,addressModeV:`clamp-to-edge`}}),r=e.createTexture({format:`depth16unorm`,width:1,height:1});this.fbo=e.createFramebuffer({id:`shadowmap`,width:1,height:1,colorAttachments:[n],depthStencilAttachment:r})}delete(){this.fbo&&=(this.fbo.destroy(),null)}getShadowMap(){return this.fbo.colorAttachments[0].texture}render(e){let t=this.fbo,n=this.device.canvasContext.cssToDeviceRatio(),r=e.viewports[0],i=r.width*n,a=r.height*n,o=[1,1,1,1];(i!==t.width||a!==t.height)&&t.resize({width:i,height:a}),super.render({...e,clearColor:o,target:t,pass:`shadow`})}getLayerParameters(e,t,n){return{...e.props.parameters,blend:!1,depthWriteEnabled:!0,depthCompare:`less-equal`}}shouldDrawLayer(e){return e.props.shadowEnabled!==!1}getShaderModuleProps(e,t,n){return{shadow:{project:n.project,drawToShadowMap:!0}}}},Wf={color:[255,255,255],intensity:1},Gf=[{color:[255,255,255],intensity:1,direction:[-1,3,-1]},{color:[255,255,255],intensity:.9,direction:[1,-8,-2.5]}],Kf=[0,0,0,200/255],qf=class{constructor(e={}){this.id=`lighting-effect`,this.shadowColor=Kf,this.shadow=!1,this.directionalLights=[],this.pointLights=[],this.shadowPasses=[],this.dummyShadowMap=null,this.setProps(e)}setup(e){this.context=e;let{device:t,deck:n}=e;this.shadow&&!this.dummyShadowMap&&(this._createShadowPasses(t),n._addDefaultShaderModule(hf),this.dummyShadowMap=t.createTexture({width:1,height:1}))}setProps(e){this.ambientLight=void 0,this.directionalLights=[],this.pointLights=[];for(let t in e){let n=e[t];switch(n.type){case`ambient`:this.ambientLight=n;break;case`directional`:this.directionalLights.push(n);break;case`point`:this.pointLights.push(n);break;default:}}this._applyDefaultLights(),this.shadow=this.directionalLights.some(e=>e.shadow),this.context&&this.setup(this.context),this.props=e}preRender({layers:e,layerFilter:t,viewports:n,onViewportActive:r,views:i}){if(this.shadow){this.shadowMatrices=this._calculateMatrices();for(let a=0;a<this.shadowPasses.length;a++)this.shadowPasses[a].render({layers:e,layerFilter:t,viewports:n,onViewportActive:r,views:i,shaderModuleProps:{shadow:{shadowLightId:a,dummyShadowMap:this.dummyShadowMap,shadowMatrices:this.shadowMatrices}}})}}getShaderModuleProps(e,t){let n=this.shadow?{project:t.project,shadowMaps:this.shadowPasses.map(e=>e.getShadowMap()),dummyShadowMap:this.dummyShadowMap,shadowColor:this.shadowColor,shadowMatrices:this.shadowMatrices}:{},r={enabled:!0,lights:this._getLights(e)},i=e.props.material;return{shadow:n,lighting:r,phongMaterial:i,gouraudMaterial:i}}cleanup(e){for(let e of this.shadowPasses)e.delete();this.shadowPasses.length=0,this.dummyShadowMap&&(this.dummyShadowMap.destroy(),this.dummyShadowMap=null,e.deck._removeDefaultShaderModule(hf))}_calculateMatrices(){let e=[];for(let t of this.directionalLights){let n=new R().lookAt({eye:new I(t.direction).negate()});e.push(n)}return e}_createShadowPasses(e){for(let t=0;t<this.directionalLights.length;t++){let n=new Uf(e);this.shadowPasses[t]=n}}_applyDefaultLights(){let{ambientLight:e,pointLights:t,directionalLights:n}=this;!e&&t.length===0&&n.length===0&&(this.ambientLight=new jf(Wf),this.directionalLights.push(new If(Gf[0]),new If(Gf[1])))}_getLights(e){let t=[];this.ambientLight&&t.push(this.ambientLight);for(let n of this.pointLights)t.push(n.getProjectedLight({layer:e}));for(let n of this.directionalLights)t.push(n.getProjectedLight({layer:e}));return t}},Jf=new class{constructor(e={}){this._pool=[],this.opts={overAlloc:2,poolSize:100},this.setOptions(e)}setOptions(e){Object.assign(this.opts,e)}allocate(e,t,{size:n=1,type:r,padding:i=0,copy:a=!1,initialize:o=!1,maxCount:s}){let c=r||e&&e.constructor||Float32Array,l=t*n+i;if(ArrayBuffer.isView(e)){if(l<=e.length)return e;if(l*e.BYTES_PER_ELEMENT<=e.buffer.byteLength)return new c(e.buffer,0,l)}let u=1/0;s&&(u=s*n+i);let d=this._allocate(c,l,o,u);return e&&a?d.set(e):o||d.fill(0,0,4),this._release(e),d}release(e){this._release(e)}_allocate(e,t,n,r){let i=Math.max(Math.ceil(t*this.opts.overAlloc),1);i>r&&(i=r);let a=this._pool,o=e.BYTES_PER_ELEMENT*i,s=a.findIndex(e=>e.byteLength>=o);if(s>=0){let t=new e(a.splice(s,1)[0],0,i);return n&&t.fill(0),t}return new e(i)}_release(e){if(!ArrayBuffer.isView(e))return;let t=this._pool,{buffer:n}=e,{byteLength:r}=n,i=t.findIndex(e=>e.byteLength>=r);i<0?t.push(n):(i>0||t.length<this.opts.poolSize)&&t.splice(i,0,n),t.length>this.opts.poolSize&&t.shift()}};function Yf(){return[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}function Xf(e,t){let n=e%t;return n<0?t+n:n}function Zf(e){return[e[12],e[13],e[14]]}function Qf(e){return{left:ep(e[3]+e[0],e[7]+e[4],e[11]+e[8],e[15]+e[12]),right:ep(e[3]-e[0],e[7]-e[4],e[11]-e[8],e[15]-e[12]),bottom:ep(e[3]+e[1],e[7]+e[5],e[11]+e[9],e[15]+e[13]),top:ep(e[3]-e[1],e[7]-e[5],e[11]-e[9],e[15]-e[13]),near:ep(e[3]+e[2],e[7]+e[6],e[11]+e[10],e[15]+e[14]),far:ep(e[3]-e[2],e[7]-e[6],e[11]-e[10],e[15]-e[14])}}var $f=new I;function ep(e,t,n,r){$f.set(e,t,n);let i=$f.len();return{distance:r/i,normal:new I(-e/i,-t/i,-n/i)}}function tp(e){return e-Math.fround(e)}var np;function rp(e,t){let{size:n=1,startIndex:r=0}=t,i=t.endIndex===void 0?e.length:t.endIndex,a=(i-r)/n;np=Jf.allocate(np,a,{type:Float32Array,size:n*2});let o=r,s=0;for(;o<i;){for(let t=0;t<n;t++){let r=e[o++];np[s+t]=r,np[s+t+n]=tp(r)}s+=n*2}return np.subarray(0,a*n*2)}function ip(e){let t=null,n=!1;for(let r of e)r&&(t?(n||=(t=[[t[0][0],t[0][1]],[t[1][0],t[1][1]]],!0),t[0][0]=Math.min(t[0][0],r[0][0]),t[0][1]=Math.min(t[0][1],r[0][1]),t[1][0]=Math.max(t[1][0],r[1][0]),t[1][1]=Math.max(t[1][1],r[1][1])):t=r);return t}var ap=Math.PI/180,op=Yf(),sp=[0,0,0],cp={unitsPerMeter:[1,1,1],metersPerUnit:[1,1,1]};function lp({width:e,height:t,orthographic:n,fovyRadians:r,focalDistance:i,padding:a,near:o,far:s}){let c=e/t,l=n?new R().orthographic({fovy:r,aspect:c,focalDistance:i,near:o,far:s}):new R().perspective({fovy:r,aspect:c,near:o,far:s});if(a){let{left:n=0,right:r=0,top:i=0,bottom:o=0}=a,s=P((n+e-r)/2,0,e)-e/2,c=P((i+t-o)/2,0,t)-t/2;l[8]-=s*2/e,l[9]+=c*2/t}return l}var up=class e{constructor(e={}){this._frustumPlanes={},this.id=e.id||this.constructor.displayName||`viewport`,this.x=e.x||0,this.y=e.y||0,this.width=e.width||1,this.height=e.height||1,this.zoom=e.zoom||0,this.padding=e.padding,this.distanceScales=e.distanceScales||cp,this.focalDistance=e.focalDistance||1,this.position=e.position||sp,this.modelMatrix=e.modelMatrix||null;let{longitude:t,latitude:n}=e;this.isGeospatial=Number.isFinite(n)&&Number.isFinite(t),this._initProps(e),this._initMatrices(e),this.equals=this.equals.bind(this),this.project=this.project.bind(this),this.unproject=this.unproject.bind(this),this.projectPosition=this.projectPosition.bind(this),this.unprojectPosition=this.unprojectPosition.bind(this),this.projectFlat=this.projectFlat.bind(this),this.unprojectFlat=this.unprojectFlat.bind(this)}get subViewports(){return null}get metersPerPixel(){return this.distanceScales.metersPerUnit[2]/this.scale}get projectionMode(){return this.isGeospatial?this.zoom<12?td.WEB_MERCATOR:td.WEB_MERCATOR_AUTO_OFFSET:td.IDENTITY}equals(t){return t instanceof e?this===t||t.width===this.width&&t.height===this.height&&t.scale===this.scale&&t.projectionMode===this.projectionMode&&t.resolution===this.resolution&&Pe(t.distanceScales.unitsPerMeter,this.distanceScales.unitsPerMeter)&&Pe(t.projectionMatrix,this.projectionMatrix)&&Pe(t.viewMatrix,this.viewMatrix):!1}project(e,{topLeft:t=!0}={}){let n=Xd(this.projectPosition(e),this.pixelProjectionMatrix),[r,i]=n,a=t?i:this.height-i;return e.length===2?[r,a]:[r,a,n[2]]}unproject(e,{topLeft:t=!0,targetZ:n}={}){let[r,i,a]=e,o=t?i:this.height-i,s=n&&n*this.distanceScales.unitsPerMeter[2],c=Zd([r,o,a],this.pixelUnprojectionMatrix,s),[l,u,d]=this.unprojectPosition(c);return Number.isFinite(a)?[l,u,d]:Number.isFinite(n)?[l,u,n]:[l,u]}projectPosition(e){let[t,n]=this.projectFlat(e);return[t,n,(e[2]||0)*this.distanceScales.unitsPerMeter[2]]}unprojectPosition(e){let[t,n]=this.unprojectFlat(e);return[t,n,(e[2]||0)*this.distanceScales.metersPerUnit[2]]}projectFlat(e){if(this.isGeospatial){let t=Bd(e);return t[1]=P(t[1],-318,830),t}return e}unprojectFlat(e){return this.isGeospatial?Vd(e):e}getBounds(e={}){let t={targetZ:e.z||0},n=this.unproject([0,0],t),r=this.unproject([this.width,0],t),i=this.unproject([0,this.height],t),a=this.unproject([this.width,this.height],t);return[Math.min(n[0],r[0],i[0],a[0]),Math.min(n[1],r[1],i[1],a[1]),Math.max(n[0],r[0],i[0],a[0]),Math.max(n[1],r[1],i[1],a[1])]}getDistanceScales(e){return e&&this.isGeospatial?Wd({longitude:e[0],latitude:e[1],highPrecision:!0}):this.distanceScales}containsPixel({x:e,y:t,width:n=1,height:r=1}){return e<this.x+this.width&&this.x<e+n&&t<this.y+this.height&&this.y<t+r}getFrustumPlanes(){return this._frustumPlanes.near||Object.assign(this._frustumPlanes,Qf(this.viewProjectionMatrix)),this._frustumPlanes}panByPosition(e,t,n){return null}_initProps(e){let t=e.longitude,n=e.latitude;this.isGeospatial&&(Number.isFinite(e.zoom)||(this.zoom=Hd({latitude:n})+Math.log2(this.focalDistance)),this.distanceScales=e.distanceScales||Wd({latitude:n,longitude:t}));let r=2**this.zoom;this.scale=r;let{position:i,modelMatrix:a}=e,o=sp;if(i&&(o=a?new R(a).transformAsVector(i,[]):i),this.isGeospatial){let e=this.projectPosition([t,n,0]);this.center=new I(o).scale(this.distanceScales.unitsPerMeter).add(e)}else this.center=this.projectPosition(o)}_initMatrices(e){let{viewMatrix:t=op,projectionMatrix:n=null,orthographic:r=!1,fovyRadians:i,fovy:a=75,near:o=.1,far:s=1e3,padding:c=null,focalDistance:l=1}=e;this.viewMatrixUncentered=t,this.viewMatrix=new R().multiplyRight(t).translate(new I(this.center).negate()),this.projectionMatrix=n||lp({width:this.width,height:this.height,orthographic:r,fovyRadians:i||a*ap,focalDistance:l,padding:c,near:o,far:s});let u=Yf();Xt(u,u,this.projectionMatrix),Xt(u,u,this.viewMatrix),this.viewProjectionMatrix=u,this.viewMatrixInverse=Jt([],this.viewMatrix)||this.viewMatrix,this.cameraPosition=Zf(this.viewMatrixInverse);let d=Yf(),f=Yf();Qt(d,d,[this.width/2,-this.height/2,1]),Zt(d,d,[1,-1,0]),Xt(f,d,this.viewProjectionMatrix),this.pixelProjectionMatrix=f,this.pixelUnprojectionMatrix=Jt(Yf(),this.pixelProjectionMatrix),this.pixelUnprojectionMatrix||z.warn(`Pixel project matrix not invertible`)()}};up.displayName=`Viewport`;var dp=class e extends up{constructor(e={}){let{latitude:t=0,longitude:n=0,zoom:r=0,pitch:i=0,bearing:a=0,nearZMultiplier:o=.1,farZMultiplier:s=1.01,nearZ:c,farZ:l,orthographic:u=!1,projectionMatrix:d,repeat:f=!1,worldOffset:p=0,position:m,padding:h,legacyMeterSizes:g=!1}=e,{width:_,height:v,altitude:y=1.5}=e,b=2**r;_||=1,v||=1;let x,S=null;if(d)y=d[5]/2,x=Jd(y);else{e.fovy?(x=e.fovy,y=Yd(x)):x=Jd(y);let n;if(h){let{top:e=0,bottom:t=0}=h;n=[0,P((e+v-t)/2,0,v)-v/2]}S=qd({width:_,height:v,scale:b,center:m&&[0,0,m[2]*Ud(t)],offset:n,pitch:i,fovy:x,nearZMultiplier:o,farZMultiplier:s}),Number.isFinite(c)&&(S.near=c),Number.isFinite(l)&&(S.far=l)}let C=Kd({height:v,pitch:i,bearing:a,scale:b,altitude:y});p&&(C=new R().translate([512*p,0,0]).multiplyLeft(C)),super({...e,width:_,height:v,viewMatrix:C,longitude:n,latitude:t,zoom:r,...S,fovy:x,focalDistance:y}),this.latitude=t,this.longitude=n,this.zoom=r,this.pitch=i,this.bearing=a,this.altitude=y,this.fovy=x,this.orthographic=u,this._subViewports=f?[]:null,this._pseudoMeters=g,Object.freeze(this)}get subViewports(){if(this._subViewports&&!this._subViewports.length){let t=this.getBounds(),n=Math.floor((t[0]+180)/360),r=Math.ceil((t[2]-180)/360);for(let t=n;t<=r;t++){let n=t?new e({...this,worldOffset:t}):this;this._subViewports.push(n)}}return this._subViewports}equals(t){return t instanceof e&&t._pseudoMeters===this._pseudoMeters&&super.equals(t)}projectPosition(e){if(this._pseudoMeters)return super.projectPosition(e);let[t,n]=this.projectFlat(e);return[t,n,(e[2]||0)*Ud(e[1])]}unprojectPosition(e){if(this._pseudoMeters)return super.unprojectPosition(e);let[t,n]=this.unprojectFlat(e);return[t,n,(e[2]||0)/Ud(n)]}addMetersToLngLat(e,t){return Gd(e,t)}panByPosition(e,t,n){let r=Zd(t,this.pixelUnprojectionMatrix),i=Ge([],this.projectFlat(e),qe([],r)),a=Ge([],this.center,i),[o,s]=this.unprojectFlat(a);return{longitude:o,latitude:s}}panByPosition3D(e,t){let n=e[2]||0,r=$e([],e,this.unproject(t,{targetZ:n}));return{longitude:this.longitude+r[0],latitude:this.latitude+r[1]}}getBounds(e={}){let t=tf(this,e.z||0);return[Math.min(t[0][0],t[1][0],t[2][0],t[3][0]),Math.min(t[0][1],t[1][1],t[2][1],t[3][1]),Math.max(t[0][0],t[1][0],t[2][0],t[3][0]),Math.max(t[0][1],t[1][1],t[2][1],t[3][1])]}fitBounds(t,n={}){let{width:r,height:i}=this,{longitude:a,latitude:o,zoom:s}=Qd({width:r,height:i,bounds:t,...n});return new e({width:r,height:i,longitude:a,latitude:o,zoom:s})}};dp.displayName=`WebMercatorViewport`;var fp=[0,0,0];function pp(e,t,n=!1){let r=t.projectPosition(e);if(n&&t instanceof dp){let[n,i,a=0]=e;r[2]=a*t.getDistanceScales([n,i]).unitsPerMeter[2]}return r}function mp(e){let{viewport:t,modelMatrix:n,coordinateOrigin:r}=e,{coordinateSystem:i,fromCoordinateSystem:a,fromCoordinateOrigin:o}=e;return i==="default"&&(i=t.isGeospatial?`lnglat`:`cartesian`),a===void 0?a=i:a==="default"&&(a=t.isGeospatial?`lnglat`:`cartesian`),o===void 0&&(o=r),{viewport:t,coordinateSystem:i,coordinateOrigin:r,modelMatrix:n,fromCoordinateSystem:a,fromCoordinateOrigin:o}}function hp(e,{viewport:t,modelMatrix:n,coordinateSystem:r,coordinateOrigin:i,offsetMode:a}){let[o,s,c=0]=e;switch(n&&([o,s,c]=bn([],[o,s,c,1],n)),r){case`default`:return hp(e,{viewport:t,modelMatrix:n,coordinateSystem:t.isGeospatial?`lnglat`:`cartesian`,coordinateOrigin:i,offsetMode:a});case`lnglat`:return pp([o,s,c],t,a);case`lnglat-offsets`:return pp([o+i[0],s+i[1],c+(i[2]||0)],t,a);case`meter-offsets`:return pp(Gd(i,[o,s,c]),t,a);case`cartesian`:return t.isGeospatial?[o+i[0],s+i[1],c+i[2]]:t.projectPosition([o,s,c]);default:throw Error(`Invalid coordinateSystem: ${r}`)}}function gp(e,t){let{viewport:n,coordinateSystem:r,coordinateOrigin:i,modelMatrix:a,fromCoordinateSystem:o,fromCoordinateOrigin:s}=mp(t),{autoOffset:c=!0}=t,{geospatialOrigin:l=fp,shaderCoordinateOrigin:u=fp,offsetMode:d=!1}=c?hd(n,r,i):{},f=hp(e,{viewport:n,modelMatrix:a,coordinateSystem:o,coordinateOrigin:s,offsetMode:d});return d&&Ct(f,f,n.projectPosition(l||u)),f}var _p={blendColorOperation:`add`,blendColorSrcFactor:`one`,blendColorDstFactor:`zero`,blendAlphaOperation:`add`,blendAlphaSrcFactor:`constant`,blendAlphaDstFactor:`zero`},vp=class extends zf{constructor(){super(...arguments),this._colorEncoderState=null}render(e){return`pickingFBO`in e?this._drawPickingBuffer(e):{decodePickingColor:null,stats:super._render(e)}}_drawPickingBuffer({layers:e,layerFilter:t,views:n,viewports:r,onViewportActive:i,pickingFBO:a,deviceRect:{x:o,y:s,width:c,height:l},cullRect:u,effects:d,pass:f=`picking`,pickZ:p,canvasContext:m,shaderModuleProps:h,clearColor:g}){this.pickZ=p;let _=this._resetColorEncoder(p),v=[o,s,c,l],y=super._render({target:a,layers:e,layerFilter:t,views:n,viewports:r,onViewportActive:i,cullRect:u,effects:d?.filter(e=>e.useInPicking),pass:f,canvasContext:m,isPicking:!0,shaderModuleProps:h,clearColor:g??[0,0,0,0],colorMask:15,scissorRect:v});return this._colorEncoderState=null,{decodePickingColor:_&&bp.bind(null,_),stats:y}}shouldDrawLayer(e){let{pickable:t,operation:n}=e.props;return t&&n.includes(`draw`)||n.includes(`terrain`)||n.includes(`mask`)}getShaderModuleProps(e,t,n){return{picking:{isActive:1,isAttribute:this.pickZ,disabledPickingIndices:e.internalState?.disabledPickingIndices},lighting:{enabled:!1}}}getLayerParameters(e,t,n){let r={...e.props.parameters},{pickable:i,operation:a}=e.props;return this._colorEncoderState?i&&a.includes(`draw`)?(Object.assign(r,_p),r.blend=!0,this.device.type===`webgpu`?r.blendConstant=yp(this._colorEncoderState,e,n):r.blendColor=yp(this._colorEncoderState,e,n),a.includes(`terrain`)&&e.state?._hasPickingCover&&(r.blendAlphaSrcFactor=`one`)):a.includes(`terrain`)&&(r.blend=!1):r.blend=!1,r}_resetColorEncoder(e){return this._colorEncoderState=e?null:{byLayer:new Map,byAlpha:[]},this._colorEncoderState}};function yp(e,t,n){let{byLayer:r,byAlpha:i}=e,a,o=r.get(t);return o?(o.viewports.push(n),a=o.a):(a=r.size+1,a<=255?(o={a,layer:t,viewports:[n]},r.set(t,o),i[a]=o):(z.warn(`Too many pickable layers, only picking the first 255`)(),a=0)),[0,0,0,a/255]}function bp(e,t){let n=e.byAlpha[t[3]];return n&&{pickedLayer:n.layer,pickedViewports:n.viewports,pickedObjectIndex:n.layer.decodePickingColor(t)}}var xp={NO_STATE:`Awaiting state`,MATCHED:`Matched. State transferred from previous layer`,INITIALIZED:`Initialized`,AWAITING_GC:`Discarded. Awaiting garbage collection`,AWAITING_FINALIZATION:`No longer matched. Awaiting garbage collection`,FINALIZED:`Finalized! Awaiting garbage collection`},Sp=Symbol.for(`component`),Cp=Symbol.for(`propTypes`),wp=Symbol.for(`deprecatedProps`),Tp=Symbol.for(`asyncPropDefaults`),Ep=Symbol.for(`asyncPropOriginal`),Dp=Symbol.for(`asyncPropResolved`);function Op(e,t=()=>!0){return Array.isArray(e)?kp(e,t,[]):t(e)?[e]:[]}function kp(e,t,n){let r=-1;for(;++r<e.length;){let i=e[r];Array.isArray(i)?kp(i,t,n):t(i)&&n.push(i)}return n}function Ap({target:e,source:t,start:n=0,count:r=1}){let i=t.length,a=r*i,o=0;for(let r=n;o<i;o++)e[r++]=t[o];for(;o<a;)o<a-o?(e.copyWithin(n+o,n,n+o),o*=2):(e.copyWithin(n+o,n,n+a-o),o=a);return e}var jp=class{constructor(e,t,n){this._loadCount=0,this._subscribers=new Set,this.id=e,this.context=n,this.setData(t)}subscribe(e){this._subscribers.add(e)}unsubscribe(e){this._subscribers.delete(e)}inUse(){return this._subscribers.size>0}delete(){}getData(){return this.isLoaded?this._error?Promise.reject(this._error):this._content:this._loader.then(()=>this.getData())}setData(e,t){if(e===this._data&&!t)return;this._data=e;let n=++this._loadCount,r=e;typeof e==`string`&&(r=sc(e)),r instanceof Promise?(this.isLoaded=!1,this._loader=r.then(e=>{this._loadCount===n&&(this.isLoaded=!0,this._error=void 0,this._content=e)}).catch(e=>{this._loadCount===n&&(this.isLoaded=!0,this._error=e||!0)})):(this.isLoaded=!0,this._error=void 0,this._content=e);for(let e of this._subscribers)e.onChange(this.getData())}},Mp=class{constructor(e){this.protocol=e.protocol||`resource://`,this._context={device:e.device,gl:e.device?.gl,resourceManager:this},this._resources={},this._consumers={},this._pruneRequest=null}contains(e){return e.startsWith(this.protocol)?!0:e in this._resources}add({resourceId:e,data:t,forceUpdate:n=!1,persistent:r=!0}){let i=this._resources[e];i?i.setData(t,n):(i=new jp(e,t,this._context),this._resources[e]=i),i.persistent=r}remove(e){let t=this._resources[e];t&&(t.delete(),delete this._resources[e])}unsubscribe({consumerId:e}){let t=this._consumers[e];if(t){for(let e in t){let n=t[e],r=this._resources[n.resourceId];r&&r.unsubscribe(n)}delete this._consumers[e],this.prune()}}subscribe({resourceId:e,onChange:t,consumerId:n,requestId:r=`default`}){let{_resources:i,protocol:a}=this;e.startsWith(a)&&(e=e.replace(a,``),i[e]||this.add({resourceId:e,data:null,persistent:!1}));let o=i[e];if(this._track(n,r,o,t),o)return o.getData()}prune(){this._pruneRequest||=setTimeout(()=>this._prune(),0)}finalize(){for(let e in this._resources)this._resources[e].delete()}_track(e,t,n,r){let i=this._consumers,a=i[e]=i[e]||{},o=a[t],s=o&&o.resourceId&&this._resources[o.resourceId];s&&(s.unsubscribe(o),this.prune()),n&&(o?(o.onChange=r,o.resourceId=n.id):o={onChange:r,resourceId:n.id},a[t]=o,n.subscribe(o))}_prune(){this._pruneRequest=null;for(let e of Object.keys(this._resources)){let t=this._resources[e];!t.persistent&&!t.inUse()&&(t.delete(),delete this._resources[e])}}},Np=`layerManager.setLayers`,Pp=`layerManager.activateViewport`,Fp=class{constructor(e,t){this._lastRenderedLayers=[],this._needsRedraw=!1,this._needsUpdate=!1,this._nextLayers=null,this._debug=!1,this._defaultShaderModulesChanged=!1,this.activateViewport=e=>{el(Pp,this,e),e&&(this.context.viewport=e)};let{deck:n,stats:r,viewport:i,timeline:a}=t||{};this.layers=[],this.resourceManager=new Mp({device:e,protocol:`deck://`}),this.context={mousePosition:null,userData:{},layerManager:this,device:e,gl:e?.gl,deck:n,shaderAssembler:Df(e?.info?.shadingLanguage||`glsl`),defaultShaderModules:[sl],renderPass:void 0,stats:r||new j({id:`deck.gl`}),viewport:i||new up({id:`DEFAULT-INITIAL-VIEWPORT`}),timeline:a||new bi,resourceManager:this.resourceManager,onError:void 0},Object.seal(this)}finalize(){this.resourceManager.finalize();for(let e of this.layers)this._finalizeLayer(e)}needsRedraw(e={clearRedrawFlags:!1}){let t=this._needsRedraw;e.clearRedrawFlags&&(this._needsRedraw=!1);for(let n of this.layers){let r=n.getNeedsRedraw(e);t||=r}return t}needsUpdate(){return this._nextLayers&&this._nextLayers!==this._lastRenderedLayers?`layers changed`:this._defaultShaderModulesChanged?`shader modules changed`:this._needsUpdate}setNeedsRedraw(e){this._needsRedraw=this._needsRedraw||e}setNeedsUpdate(e){this._needsUpdate=this._needsUpdate||e}getLayers({layerIds:e}={}){return e?this.layers.filter(t=>e.find(e=>t.id.indexOf(e)===0)):this.layers}setProps(e){`debug`in e&&(this._debug=e.debug),`userData`in e&&(this.context.userData=e.userData),`layers`in e&&(this._nextLayers=e.layers),`onError`in e&&(this.context.onError=e.onError)}setLayers(e,t){el(Np,this,t,e),this._lastRenderedLayers=e;let n=Op(e,Boolean);for(let e of n)e.context=this.context;this._updateLayers(this.layers,n)}updateLayers(){let e=this.needsUpdate();e&&(this.setNeedsRedraw(`updating layers: ${e}`),this.setLayers(this._nextLayers||this._lastRenderedLayers,e)),this._nextLayers=null}addDefaultShaderModule(e){let{defaultShaderModules:t}=this.context;t.find(t=>t.name===e.name)||(t.push(e),this._defaultShaderModulesChanged=!0)}removeDefaultShaderModule(e){let{defaultShaderModules:t}=this.context,n=t.findIndex(t=>t.name===e.name);n>=0&&(t.splice(n,1),this._defaultShaderModulesChanged=!0)}_handleError(e,t,n){n.raiseError(t,`${e} of ${n}`)}_updateLayers(e,t){let n={};for(let t of e)n[t.id]?z.warn(`Multiple old layers with same id ${t.id}`)():n[t.id]=t;if(this._defaultShaderModulesChanged){for(let t of e)t.setNeedsUpdate(),t.setChangeFlags({extensionsChanged:!0});this._defaultShaderModulesChanged=!1}let r=[];this._updateSublayersRecursively(t,n,r),this._finalizeOldLayers(n);let i=!1;for(let e of r)if(e.hasUniformTransition()){i=`Uniform transition in ${e}`;break}this._needsUpdate=i,this.layers=r}_updateSublayersRecursively(e,t,n){for(let r of e){r.context=this.context;let e=t[r.id];e===null&&z.warn(`Multiple new layers with same id ${r.id}`)(),t[r.id]=null;let i=null;try{this._debug&&e!==r&&r.validateProps(),e?(this._transferLayerState(e,r),this._updateLayer(r)):this._initializeLayer(r),n.push(r),i=r.isComposite?r.getSubLayers():null}catch(e){this._handleError(`matching`,e,r)}i&&this._updateSublayersRecursively(i,t,n)}}_finalizeOldLayers(e){for(let t in e){let n=e[t];n&&this._finalizeLayer(n)}}_initializeLayer(e){try{e._initialize(),e.lifecycle=xp.INITIALIZED}catch(t){this._handleError(`initialization`,t,e)}}_transferLayerState(e,t){t._transferState(e),t.lifecycle=xp.MATCHED,t!==e&&(e.lifecycle=xp.AWAITING_GC)}_updateLayer(e){try{e._update()}catch(t){this._handleError(`update`,t,e)}}_finalizeLayer(e){this._needsRedraw=this._needsRedraw||`finalized ${e}`,e.lifecycle=xp.AWAITING_FINALIZATION;try{e._finalize(),e.lifecycle=xp.FINALIZED}catch(t){this._handleError(`finalization`,t,e)}}};function Ip(e,t,n){if(e===t)return!0;if(!n||!e||!t)return!1;if(Array.isArray(e)){if(!Array.isArray(t)||e.length!==t.length)return!1;for(let r=0;r<e.length;r++)if(!Ip(e[r],t[r],n-1))return!1;return!0}if(Array.isArray(t))return!1;if(typeof e==`object`&&typeof t==`object`){let r=Object.keys(e),i=Object.keys(t);if(r.length!==i.length)return!1;for(let i of r)if(!t.hasOwnProperty(i)||!Ip(e[i],t[i],n-1))return!1;return!0}return!1}var Lp=`default-canvas`,Rp=class{constructor(e){this.views=[],this.width=100,this.height=100,this.viewState={},this.controllers={},this.timeline=e.timeline,this._viewports=[],this._viewportMap={},this._isUpdating=!1,this._needsRedraw=`First render`,this._needsUpdate=`Initialize`,this._eventManager=e.eventManager,this._eventManagers=e.eventManagers||{},this._viewEventManagers={},this._eventCallbacks={onViewStateChange:e.onViewStateChange,onInteractionStateChange:e.onInteractionStateChange},this._pickPosition=e.pickPosition,this._getCanvasContext=e.getCanvasContext,Object.seal(this),this.setProps(e)}finalize(){for(let e in this.controllers){let t=this.controllers[e];t&&t.finalize()}this.controllers={}}needsRedraw(e={clearRedrawFlags:!1}){let t=this._needsRedraw;return e.clearRedrawFlags&&(this._needsRedraw=!1),t}setNeedsUpdate(e){this._needsUpdate=this._needsUpdate||e,this._needsRedraw=this._needsRedraw||e}updateViewStates(){for(let e in this.controllers){let t=this.controllers[e];t&&t.updateTransition()}}getViewports(e){return e?this._viewports.filter(t=>{let n=!e.canvasId||this.getCanvasId(t.id)===e.canvasId,r=!(`x`in e)||t.containsPixel(e);return n&&r}):this._viewports}getViews(){let e={};return this.views.forEach(t=>{e[t.id]=t}),e}getView(e){return this.views.find(t=>t.id===e)}getViewState(e){let t=typeof e==`string`?this.getView(e):e,n=t&&this.viewState[t.getViewStateId()]||this.viewState;return t?t.filterViewState(n):n}getViewport(e){return this._viewportMap[e]}getCanvasId(e){let t=typeof e==`string`?this.getView(e):e;return t?this._viewEventManagers[t.id]?.canvasId||this._getCanvasIdFromView(t):void 0}unproject(e,t){let n=this.getViewports(),r={x:e[0],y:e[1]};for(let i=n.length-1;i>=0;--i){let a=n[i];if(a.containsPixel(r)){let n=e.slice();return n[0]-=a.x,n[1]-=a.y,a.unproject(n,t)}}return null}setProps(e){e.views&&this._setViews(e.views),e.viewState&&this._setViewState(e.viewState),(`width`in e||`height`in e)&&this._setSize(e.width,e.height),`pickPosition`in e&&(this._pickPosition=e.pickPosition),`eventManagers`in e&&this._setEventManagers(e.eventManagers||{}),this._isUpdating||this._update()}_update(){this._isUpdating=!0,this._needsUpdate&&(this._needsUpdate=!1,this._rebuildViewports()),this._needsUpdate&&(this._needsUpdate=!1,this._rebuildViewports()),this._isUpdating=!1}_setSize(e,t){(e!==this.width||t!==this.height)&&(this.width=e,this.height=t,this.setNeedsUpdate(`Size changed`))}_setViews(e){e=Op(e,Boolean),this._diffViews(e,this.views)&&this.setNeedsUpdate(`views changed`),this.views=e}_setViewState(e){e?(Ip(e,this.viewState,3)||this.setNeedsUpdate(`viewState changed`),this.viewState=e):z.warn("missing `viewState` or `initialViewState`")()}_setEventManagers(e){this._eventManagers!==e&&(this._eventManagers=e,this.setNeedsUpdate(`eventManagers changed`))}_getCanvasIdFromView(e){return e.props.canvasId||this._getCanvasContext?.(e.id)?.id||`default-canvas`}_getCanvasDimensions(e){let[t,n]=(this._getCanvasContext?.(e.id))?.getCSSSize()||[this.width,this.height];return{width:t,height:n}}_getViewEventManager(e){let t=this.getCanvasId(e)||`default-canvas`;return{canvasId:t,eventManager:this._eventManagers[t]||this._eventManager}}_startViewportRebuild(){let e=this.controllers,t=this._viewEventManagers;return this._viewports=[],this.controllers={},this._viewEventManagers={},{oldControllers:e,oldViewEventManagers:t}}_getReusableController(e,t,n){return e&&(t?.canvasId!==n.canvasId||t?.eventManager!==n.eventManager)?(e.finalize(),null):e}_createController(e,t){let n=t.type;return new n({timeline:this.timeline,eventManager:this._getViewEventManager(e).eventManager,onViewStateChange:this._eventCallbacks.onViewStateChange,onStateChange:this._eventCallbacks.onInteractionStateChange,makeViewport:t=>this.getView(e.id)?.makeViewport({viewState:t,...this._getCanvasDimensions(e)}),pickPosition:(t,n)=>this._pickPosition?.(t,n,e.id)})}_updateController(e,t,n,r){let i=e.controller;if(i&&n){let a={...t,...i,id:e.id,x:n.x,y:n.y,width:n.width,height:n.height};return(!r||r.constructor!==i.type)&&(r=this._createController(e,a)),r&&r.setProps(a),r}return null}_rebuildViewports(){let{views:e}=this,{oldControllers:t,oldViewEventManagers:n}=this._startViewportRebuild(),r=!1;for(let i=e.length;i--;){let a=e[i],{width:o,height:s}=this._getCanvasDimensions(a),c=this._getViewEventManager(a);this._viewEventManagers[a.id]=c;let l=this.getViewState(a),u=a.makeViewport({viewState:l,width:o,height:s}),d=this._getReusableController(t[a.id],n[a.id],c),f=!!a.controller;f&&!d&&(r=!0),(r||!f)&&d&&(d.finalize(),d=null),this.controllers[a.id]=this._updateController(a,l,u,d),u&&this._viewports.unshift(u)}for(let e in t){let n=t[e];n&&!this.controllers[e]&&n.finalize()}this._buildViewportMap()}_buildViewportMap(){this._viewportMap={},this._viewports.forEach(e=>{e.id&&(this._viewportMap[e.id]=this._viewportMap[e.id]||e)})}_diffViews(e,t){return e.length!==t.length||e.some((n,r)=>!e[r].equals(t[r]))}},zp=/^(?:\d+\.?\d*|\.\d+)$/;function Bp(e){switch(typeof e){case`number`:if(!Number.isFinite(e))throw Error(`Could not parse position string ${e}`);return{type:`literal`,value:e};case`string`:try{return new Wp(Up(e)).parseExpression()}catch(t){let n=t instanceof Error?t.message:String(t);throw Error(`Could not parse position string ${e}: ${n}`)}default:throw Error(`Could not parse position string ${e}`)}}function Vp(e,t){switch(e.type){case`literal`:return e.value;case`percentage`:return Math.round(e.value*t);case`binary`:let n=Vp(e.left,t),r=Vp(e.right,t);return e.operator===`+`?n+r:n-r;default:throw Error(`Unknown layout expression type`)}}function Hp(e,t){return Vp(e,t)}function Up(e){let t=[],n=0;for(;n<e.length;){let r=e[n];if(/\s/.test(r)){n++;continue}if(r===`+`||r===`-`||r===`(`||r===`)`||r===`%`){t.push({type:`symbol`,value:r}),n++;continue}if(Gp(r)||r===`.`){let i=n,a=r===`.`;for(n++;n<e.length;){let t=e[n];if(Gp(t)){n++;continue}if(t===`.`&&!a){a=!0,n++;continue}break}let o=e.slice(i,n);if(!zp.test(o))throw Error(`Invalid number token`);t.push({type:`number`,value:parseFloat(o)});continue}if(Kp(r)){let r=n;for(;n<e.length&&Kp(e[n]);)n++;let i=e.slice(r,n).toLowerCase();t.push({type:`word`,value:i});continue}throw Error(`Invalid token in position string`)}return t}var Wp=class{constructor(e){this.index=0,this.tokens=e}parseExpression(){let e=this.parseBinaryExpression();if(this.index<this.tokens.length)throw Error(`Unexpected token at end of expression`);return e}parseBinaryExpression(){let e=this.parseFactor(),t=this.peek();for(;qp(t);){this.index++;let n=this.parseFactor();e={type:`binary`,operator:t.value,left:e,right:n},t=this.peek()}return e}parseFactor(){let e=this.peek();if(!e)throw Error(`Unexpected end of expression`);if(e.type===`symbol`&&e.value===`+`)return this.index++,this.parseFactor();if(e.type===`symbol`&&e.value===`-`)return this.index++,{type:`binary`,operator:`-`,left:{type:`literal`,value:0},right:this.parseFactor()};if(e.type===`symbol`&&e.value===`(`){this.index++;let e=this.parseBinaryExpression();if(!this.consumeSymbol(`)`))throw Error(`Missing closing parenthesis`);return e}if(e.type===`word`&&e.value===`calc`){if(this.index++,!this.consumeSymbol(`(`))throw Error(`Missing opening parenthesis after calc`);let e=this.parseBinaryExpression();if(!this.consumeSymbol(`)`))throw Error(`Missing closing parenthesis`);return e}if(e.type===`number`){this.index++;let t=e.value,n=this.peek();return n&&n.type===`symbol`&&n.value===`%`?(this.index++,{type:`percentage`,value:t/100}):(n&&n.type===`word`&&n.value===`px`&&this.index++,{type:`literal`,value:t})}throw Error(`Unexpected token in expression`)}consumeSymbol(e){let t=this.peek();return t&&t.type===`symbol`&&t.value===e?(this.index++,!0):!1}peek(){return this.tokens[this.index]||null}};function Gp(e){return e>=`0`&&e<=`9`}function Kp(e){return e>=`a`&&e<=`z`||e>=`A`&&e<=`Z`}function qp(e){return!!(e&&e.type===`symbol`&&(e.value===`+`||e.value===`-`))}function Jp(e,t){let n={...e};for(let e in t)e!==`id`&&(Array.isArray(n[e])&&Array.isArray(t[e])?n[e]=Yp(n[e],t[e]):n[e]=t[e]);return n}function Yp(e,t){e=e.slice();for(let n=0;n<t.length;n++){let r=t[n];Number.isFinite(r)&&(e[n]=r)}return e}var Xp=class{constructor(e){let{id:t,x:n=0,y:r=0,width:i=`100%`,height:a=`100%`,padding:o=null}=e;this.id=t||this.constructor.displayName||`view`,this.props={...e,id:this.id},this._x=Bp(n),this._y=Bp(r),this._width=Bp(i),this._height=Bp(a),this._padding=o&&{left:Bp(o.left||0),right:Bp(o.right||0),top:Bp(o.top||0),bottom:Bp(o.bottom||0)},this.equals=this.equals.bind(this),Object.seal(this)}equals(e){return this===e||this.constructor===e.constructor&&Ip(this.props,e.props,2)}clone(e){let t=this.constructor;return new t({...this.props,...e})}makeViewport({width:e,height:t,viewState:n}){n=this.filterViewState(n);let r=this.getDimensions({width:e,height:t});return!r.height||!r.width?null:new(this.getViewportType(n))({...n,...this.props,...r})}getViewStateId(){let{viewState:e}=this.props;return typeof e==`string`?e:e?.id||this.id}filterViewState(e){return this.props.viewState&&typeof this.props.viewState==`object`?this.props.viewState.id?Jp(e,this.props.viewState):this.props.viewState:e}getDimensions({width:e,height:t}){let n={x:Hp(this._x,e),y:Hp(this._y,t),width:Hp(this._width,e),height:Hp(this._height,t)};return this._padding&&(n.padding={left:Hp(this._padding.left,e),top:Hp(this._padding.top,t),right:Hp(this._padding.right,e),bottom:Hp(this._padding.bottom,t)}),n}get controller(){let e=this.props.controller;return e?e===!0?{type:this.ControllerType}:typeof e==`function`?{type:e}:{type:this.ControllerType,...e}:null}},Zp=class{constructor(e){this._inProgress=!1,this._handle=null,this.time=0,this.settings={duration:0},this._timeline=e}get inProgress(){return this._inProgress}start(e){this.cancel(),this.settings=e,this._inProgress=!0,this.settings.onStart?.(this)}end(){this._inProgress&&(this._timeline.removeChannel(this._handle),this._handle=null,this._inProgress=!1,this.settings.onEnd?.(this))}cancel(){this._inProgress&&=(this.settings.onInterrupt?.(this),this._timeline.removeChannel(this._handle),this._handle=null,!1)}update(){if(!this._inProgress)return!1;if(this._handle===null){let{_timeline:e,settings:t}=this;this._handle=e.addChannel({delay:e.getTime(),duration:t.duration})}return this.time=this._timeline.getTime(this._handle),this._onUpdate(),this.settings.onUpdate?.(this),this._timeline.isFinished(this._handle)&&this.end(),!0}_onUpdate(){}},Qp=()=>{},$p={mode:`preserve`},em={mode:`hard`},tm={BREAK:1,SNAP_TO_END:2,IGNORE:3},nm=e=>e,rm=tm.BREAK,im=class{constructor(e){this._onTransitionUpdate=e=>{let{time:t,settings:{interpolator:n,startProps:r,endProps:i,duration:a,easing:o}}=e,s=o(t/a),c=n.interpolateProps(r,i,s);this.propsInTransition=this.getControllerState({...this.props,...c},$p).getViewportProps(),this.onViewStateChange({viewState:this.propsInTransition,oldViewState:this.props})},this.getControllerState=e.getControllerState,this.propsInTransition=null,this.transition=new Zp(e.timeline),this.onViewStateChange=e.onViewStateChange||Qp,this.onStateChange=e.onStateChange||Qp}finalize(){this.transition.cancel()}getViewportInTransition(){return this.propsInTransition}processViewStateChange(e){let t=!1,n=this.props;if(this.props=e,!n||this._shouldIgnoreViewportChange(n,e))return!1;if(this._isTransitionEnabled(e)){let r=n;if(this.transition.inProgress){let{interruption:e,endProps:t}=this.transition.settings;r={...n,...e===tm.SNAP_TO_END?t:this.propsInTransition||n}}this._triggerTransition(r,e),t=!0}else this.transition.cancel();return t}updateTransition(){this.transition.update()}_isTransitionEnabled(e){let{transitionDuration:t,transitionInterpolator:n}=e;return(t>0||t===`auto`)&&!!n}_isUpdateDueToCurrentTransition(e){return this.transition.inProgress&&this.propsInTransition?this.transition.settings.interpolator.arePropsEqual(e,this.propsInTransition):!1}_shouldIgnoreViewportChange(e,t){return this.transition.inProgress?this.transition.settings.interruption===tm.IGNORE||this._isUpdateDueToCurrentTransition(t):!this._isTransitionEnabled(t)||t.transitionInterpolator.arePropsEqual(e,t)}_triggerTransition(e,t){let n=this.getControllerState(e,$p),r=this.getControllerState(t,em).shortestPathFrom(n),i=t.transitionInterpolator,a=i.getDuration?i.getDuration(e,t):t.transitionDuration;if(a===0)return;let o=i.initializeProps(e,r);this.propsInTransition={};let s={duration:a,easing:t.transitionEasing||nm,interpolator:i,interruption:t.transitionInterruption||rm,startProps:o.start,endProps:o.end,onStart:t.onTransitionStart,onUpdate:this._onTransitionUpdate,onInterrupt:this._onTransitionEnd(t.onTransitionInterrupt),onEnd:this._onTransitionEnd(t.onTransitionEnd)};this.transition.start(s),this.onStateChange({inTransition:!0}),this.updateTransition()}_onTransitionEnd(e){return t=>{this.propsInTransition=null,this.onStateChange({inTransition:!1,isZooming:!1,isPanning:!1,isRotating:!1}),e?.(t)}}};function H(e,t){if(!e)throw Error(t||`deck.gl: assertion failed.`)}var am=class{constructor(e){let{compare:t,extract:n,required:r}=e;this._propsToCompare=t,this._propsToExtract=n||t,this._requiredProps=r}arePropsEqual(e,t){for(let n of this._propsToCompare)if(!(n in e)||!(n in t)||!Pe(e[n],t[n]))return!1;return!0}initializeProps(e,t){let n={},r={};for(let i of this._propsToExtract)(i in e||i in t)&&(n[i]=e[i],r[i]=t[i]);return this._checkRequiredProps(n),this._checkRequiredProps(r),{start:n,end:r}}getDuration(e,t){return t.transitionDuration}_checkRequiredProps(e){this._requiredProps&&this._requiredProps.forEach(t=>{let n=e[t];H(Number.isFinite(n)||Array.isArray(n),`${t} is required for transition`)})}},om=[`longitude`,`latitude`,`zoom`,`bearing`,`pitch`],sm=[`longitude`,`latitude`,`zoom`],cm=class extends am{constructor(e={}){let t=Array.isArray(e)?e:e.transitionProps,n=Array.isArray(e)?{}:e;n.transitionProps=Array.isArray(t)?{compare:t,required:t}:t||{compare:om,required:sm},super(n.transitionProps),this.opts=n}initializeProps(e,t){let n=super.initializeProps(e,t),{makeViewport:r,around:i}=this.opts;if(r&&i){let a=r(e),o=r(t),s=a.unproject(i);n.start.around=i,Object.assign(n.end,{around:o.project(s),aroundPosition:s,width:t.width,height:t.height})}return n}interpolateProps(e,t,n){let r={};for(let i of this._propsToExtract)r[i]=Ne(e[i]||0,t[i]||0,n);if(t.aroundPosition&&this.opts.makeViewport){let i=this.opts.makeViewport({...t,...r});Object.assign(r,i.panByPosition(t.aroundPosition,Ne(e.around,t.around,n)))}return r}},lm={transitionDuration:0},um=300,dm=300,fm=e=>1-(1-e)*(1-e),pm=e=>e===1?1:1-2**(-10*e),mm={WHEEL:[`wheel`],PAN:[`panstart`,`panmove`,`panend`],PINCH:[`pinchstart`,`pinchmove`,`pinchend`],MULTI_PAN:[`multipanstart`,`multipanmove`,`multipanend`],DOUBLE_CLICK:[`dblclick`],DOUBLE_CLICK_DRAG:[`dblclickdragstart`,`dblclickdragmove`,`dblclickdragend`,`dblclickdragcancel`],KEYBOARD:[`keydown`]},hm={},gm=class{constructor(e){this.state={},this._events={},this._interactionState={isDragging:!1},this._customEvents=[],this._eventStartBlocked=null,this._panMove=!1,this._multiPanMode=null,this._multiPanStartCenter=null,this._doubleClickDragAnchor=null,this._suppressDoubleClickUntil=0,this.invertPan=!1,this.dragMode=`rotate`,this.inertia=0,this.scrollZoom=!0,this.dragPan=!0,this.dragRotate=!0,this.doubleClickZoom=!0,this.doubleClickDragZoom=!0,this.touchZoom=!0,this.touchRotate=!1,this.multiTouchDrag=null,this.trackpadGesture=!1,this.zoomAround=`pointer`,this.keyboard=!0,this.transitionManager=new im({...e,getControllerState:(t,n)=>new this.ControllerState({...t,constraintContext:n,makeViewport:e.makeViewport}),onViewStateChange:this._onTransition.bind(this),onStateChange:this._setInteractionState.bind(this)}),this.handleEvent=this.handleEvent.bind(this),this.eventManager=e.eventManager,this.onViewStateChange=e.onViewStateChange||(()=>{}),this.onStateChange=e.onStateChange||(()=>{}),this.makeViewport=e.makeViewport,this.pickPosition=e.pickPosition}set events(e){this.toggleEvents(this._customEvents,!1),this.toggleEvents(e,!0),this._customEvents=e,this.props&&this.setProps(this.props)}finalize(){for(let e in this._events)this._events[e]&&this.eventManager?.off(e,this.handleEvent);this.transitionManager.finalize()}handleEvent(e){this._controllerState=void 0;let t=this._eventStartBlocked;switch(e.type){case`panstart`:return!t&&this._onPanStart(e);case`panmove`:return this._onPan(e);case`panend`:return this._onPanEnd(e);case`pinchstart`:return t||!this._isTrackpadGestureAllowed(e)?!1:this._onPinchStart(e);case`pinchmove`:return this._isTrackpadGestureAllowed(e)?this._onPinch(e):!1;case`pinchend`:return this._isTrackpadGestureAllowed(e)?this._onPinchEnd(e):!1;case`multipanstart`:return!t&&this._onMultiPanStart(e);case`multipanmove`:return this._onMultiPan(e);case`multipanend`:return this._onMultiPanEnd(e);case`dblclick`:return this._onDoubleClick(e);case`dblclickdragstart`:return!t&&this._onDoubleClickDragStart(e);case`dblclickdragmove`:return this._onDoubleClickDrag(e);case`dblclickdragend`:case`dblclickdragcancel`:return this._onDoubleClickDragEnd(e);case`wheel`:return this._onWheel(e);case`keydown`:return this._onKeyDown(e);default:return!1}}get controllerState(){return this._controllerState=this._controllerState||new this.ControllerState({makeViewport:this.makeViewport,...this.props,...this.state}),this._controllerState}getCenter(e){let{x:t,y:n}=this.props,{offsetCenter:r}=e;return[r.x-t,r.y-n]}getZoomPosition(e){if(this.zoomAround===`pointer`)return e;let t=this.makeViewport(this.controllerState.getViewportProps()),[n,r]=Xd(t.center,t.pixelProjectionMatrix);return[n,r]}isPointInBounds(e,t){let{width:n,height:r}=this.props;if(t&&t.handled)return!1;let i=e[0]>=0&&e[0]<=n&&e[1]>=0&&e[1]<=r;return i&&t&&t.stopPropagation(),i}isFunctionKeyPressed(e){let{srcEvent:t}=e;return!!(t.metaKey||t.altKey||t.ctrlKey||t.shiftKey)}isDragging(){return this._interactionState.isDragging||!1}blockEvents(e){let t=setTimeout(()=>{this._eventStartBlocked===t&&(this._eventStartBlocked=null)},e);this._eventStartBlocked=t}setProps(e){e.maxBoundsPadding===void 0&&(e.maxBoundsPadding=null),e.dragMode&&(this.dragMode=e.dragMode);let t=this.props;this.props=e,`transitionInterpolator`in e||(e.transitionInterpolator=this._getTransitionProps().transitionInterpolator),this.transitionManager.processViewStateChange(e);let{inertia:n}=e;this.inertia=Number.isFinite(n)?n:n===!0?um:0;let{scrollZoom:r=!0,dragPan:i=!0,dragRotate:a=!0,doubleClickZoom:o=!0,doubleClickDragZoom:s=!1,touchZoom:c=!0,touchRotate:l=!1,multiTouchDrag:u=l?`rotate`:null,trackpadGesture:d=!1,zoomAround:f=`pointer`,keyboard:p=!0}=e,m=!!this.onViewStateChange;if(this.toggleEvents(mm.WHEEL,m&&r),this.toggleEvents(mm.PAN,m),this.toggleEvents(mm.PINCH,m&&(c||u===`rotate`)),this.toggleEvents(mm.MULTI_PAN,m&&!!u),this.toggleEvents(mm.DOUBLE_CLICK,m&&o),this.toggleEvents(mm.DOUBLE_CLICK_DRAG,m&&s),this.toggleEvents(mm.KEYBOARD,m&&p),this.scrollZoom=r,this.dragPan=i,this.dragRotate=a,this.doubleClickZoom=o,this.doubleClickDragZoom=s,this.touchZoom=c,this.touchRotate=u===`rotate`,this.multiTouchDrag=u,this.trackpadGesture=d,this.zoomAround=f,this.keyboard=p,(!t||t.height!==e.height||t.width!==e.width||t.maxBounds!==e.maxBounds||t.maxBoundsPadding!==e.maxBoundsPadding)&&e.maxBounds){let t=new this.ControllerState({...e,makeViewport:this.makeViewport}),n=t.getViewportProps();Object.keys(n).some(t=>!Ip(n[t],e[t],1))&&this.updateViewport(t)}}updateTransition(){this.transitionManager.updateTransition()}toggleEvents(e,t){this.eventManager&&e.forEach(e=>{this._events[e]!==t&&(this._events[e]=t,t?this.eventManager.on(e,this.handleEvent):this.eventManager.off(e,this.handleEvent))})}updateViewport(e,t=null,n={}){let r={...e.getViewportProps(),...t},i=this.controllerState!==e;if(this.state=e.getState(),this._setInteractionState(n),i){let e=this.controllerState&&this.controllerState.getViewportProps();this.onViewStateChange&&this.onViewStateChange({viewState:r,interactionState:this._interactionState,oldViewState:e,viewId:this.props.id})}}_onTransition(e){this.onViewStateChange({...e,interactionState:this._interactionState,viewId:this.props.id})}_setInteractionState(e){Object.assign(this._interactionState,e),this.onStateChange(this._interactionState)}_getConstraintContext(e,t){return this.props.rubberBand?{mode:t===`update`?`elastic`:t===`end`?`rebound`:`hard`}:{mode:`hard`}}_getReboundTransition(e,t){if(e.mode!==`rebound`)return null;let n=t.getViewportProps();return Object.keys(n).some(e=>!Ip(this.props[e],n[e],1))?{...this._getTransitionProps(),transitionDuration:dm,transitionEasing:pm}:null}_onPanStart(e){let t=this.getCenter(e);if(!this.isPointInBounds(t,e))return!1;let n=this.isFunctionKeyPressed(e)||e.rightButton||!1;(this.invertPan||this.dragMode===`pan`)&&(n=!n);let r=n?`pan`:`rotate`,i=this._getConstraintContext(r,`start`),a=n?this.controllerState.panStart({pos:t},i):this.controllerState.rotateStart({pos:t},i);return this._panMove=n,this.updateViewport(a,lm,{isDragging:!0}),!0}_onPan(e){return this.isDragging()?this._panMove?this._onPanMove(e):this._onPanRotate(e):!1}_onPanEnd(e){return this.isDragging()?this._panMove?this._onPanMoveEnd(e):this._onPanRotateEnd(e):!1}_onPanMove(e){if(!this.dragPan)return!1;let t=this.getCenter(e),n=this.controllerState.pan({pos:t},this._getConstraintContext(`pan`,`update`));return this.updateViewport(n,lm,{isDragging:!0,isPanning:!0}),!0}_onPanMoveEnd(e){let{inertia:t}=this;if(this.dragPan&&t&&e.velocity){let n=this.getCenter(e),r=[n[0]+e.velocityX*t/2,n[1]+e.velocityY*t/2],i=this.controllerState.pan({pos:r}).panEnd();this.updateViewport(i,{...this._getTransitionProps(),transitionDuration:t,transitionEasing:fm},{isDragging:!1,isPanning:!0})}else{let e=this.controllerState,t=this._getConstraintContext(`pan`,`end`),n=e.panEnd(t),r=this._getReboundTransition(t,n);this.updateViewport(n,r,{isDragging:!1,isPanning:!!r})}return!0}_onPanRotate(e){if(!this.dragRotate)return!1;let t=this.getCenter(e),n=this.controllerState.rotate({pos:t},this._getConstraintContext(`rotate`,`update`));return this.updateViewport(n,lm,{isDragging:!0,isRotating:!0}),!0}_onPanRotateEnd(e){let{inertia:t}=this;if(this.dragRotate&&t&&e.velocity){let n=this.getCenter(e),r=[n[0]+e.velocityX*t/2,n[1]+e.velocityY*t/2],i=this.controllerState.rotate({pos:r}).rotateEnd();this.updateViewport(i,{...this._getTransitionProps(),transitionDuration:t,transitionEasing:fm},{isDragging:!1,isRotating:!0})}else{let e=this.controllerState,t=this._getConstraintContext(`rotate`,`end`),n=e.rotateEnd(t),r=this._getReboundTransition(t,n);this.updateViewport(n,r,{isDragging:!1,isRotating:!!r})}return!0}_onWheel(e){if(!this.scrollZoom||this.trackpadGesture&&e.device!==`mouse`)return!1;let t=this.getCenter(e);if(!this.isPointInBounds(t,e))return!1;e.srcEvent.preventDefault();let{speed:n=.01,smooth:r=!1}=this.scrollZoom===!0?{}:this.scrollZoom,{delta:i}=e,a=2/(1+Math.exp(-Math.abs(i*n)));i<0&&a!==0&&(a=1/a);let o=this.getZoomPosition(t),s=r?{...this._getTransitionProps({around:o}),transitionDuration:250}:lm,c=this.controllerState.zoom({pos:o,scale:a});return this.updateViewport(c,s,{isZooming:!0,isPanning:!0}),r||this._setInteractionState({isZooming:!1,isPanning:!1}),!0}_onMultiPanStart(e){let{multiTouchDrag:t}=this;if(!t||!this._isMultiPanEventAllowed(e,t))return!1;let n=e.offsetCenter;if(!this.isPointInBounds(this.getCenter(e),e))return!1;let r=e.pointerType===`trackpad`,i={x:n.x-(r?0:e.deltaX),y:n.y-(r?0:e.deltaY)},a={...e,offsetCenter:i},o=this.getCenter(a),s=t===`pan`?this.controllerState.panStart({pos:o},this._getConstraintContext(`pan`,`start`)):this.controllerState.rotateStart({pos:o},this._getConstraintContext(`rotate`,`start`));return this._multiPanMode=t,this._multiPanStartCenter=i,this.updateViewport(s,lm,{isDragging:!0}),!0}_onMultiPan(e){let{mode:t,event:n}=this._getMultiPanEvent(e);return!t||!n||!this.isDragging()?!1:t===`pan`?this._onPanMove(n):this._onPanRotate(n)}_onMultiPanEnd(e){let{mode:t,event:n}=this._getMultiPanEvent(e);if(!t||!n||!this.isDragging())return this._resetMultiPan(),!1;let r=t===`pan`?this._onPanMoveEnd(n):this._onPanRotateEnd(n);return this._resetMultiPan(),r}_isTrackpadGestureAllowed(e){return e.pointerType!==`trackpad`||this.trackpadGesture}_isMultiPanEventAllowed(e,t){return e.pointerType===`trackpad`?this.trackpadGesture&&(t===`pan`?this.dragPan:this.dragRotate):e.pointerType===`touch`&&(t===`pan`?this.dragPan:this.dragRotate)}_getMultiPanEvent(e){let t=this._multiPanMode,n=this._multiPanStartCenter;return!t||!n?{mode:null,event:null}:{mode:t,event:{...e,offsetCenter:{x:n.x+e.deltaX,y:n.y+e.deltaY}}}}_resetMultiPan(){this._multiPanMode=null,this._multiPanStartCenter=null}_onPinchStart(e){this._doubleClickDragAnchor=null;let t=this.getCenter(e);if(!this.isPointInBounds(t,e))return!1;let n=this.controllerState.zoomStart({pos:this.getZoomPosition(t)},this._getConstraintContext(`zoom`,`start`)).rotateStart({pos:t},this._getConstraintContext(`rotate`,`start`));return hm._startPinchRotation=e.rotation,hm._lastPinchEvent=e,this.updateViewport(n,lm,{isDragging:!0}),!0}_onPinch(e){if(!this.touchZoom&&!this.touchRotate||!this.isDragging())return!1;let t=this.controllerState;if(this.touchZoom){let{scale:n}=e,r=this.getCenter(e);t=t.zoom({pos:this.getZoomPosition(r),scale:n},this._getConstraintContext(`zoom`,`update`))}if(this.touchRotate){let{rotation:n}=e;t=t.rotate({deltaAngleX:hm._startPinchRotation-n},this._getConstraintContext(`rotate`,`update`))}return this.updateViewport(t,lm,{isDragging:!0,isPanning:this.touchZoom,isZooming:this.touchZoom,isRotating:this.touchRotate}),hm._lastPinchEvent=e,!0}_onPinchEnd(e){if(!this.isDragging())return!1;let{inertia:t}=this,{_lastPinchEvent:n}=hm;if(this.touchZoom&&t&&n&&e.scale!==n.scale){let r=this.getCenter(e),i=this.getZoomPosition(r),a=this.controllerState.rotateEnd(),o=Math.log2(e.scale),s=2**(o+(o-Math.log2(n.scale))/(e.deltaTime-n.deltaTime)*t/2);a=a.zoom({pos:i,scale:s}).zoomEnd(),this.updateViewport(a,{...this._getTransitionProps({around:i}),transitionDuration:t,transitionEasing:fm},{isDragging:!1,isPanning:this.touchZoom,isZooming:this.touchZoom,isRotating:!1}),this.blockEvents(t)}else{let e=this.controllerState,t=this._getConstraintContext(`zoom`,`end`),n=this._getConstraintContext(`rotate`,`end`),r=e.zoomEnd(t).rotateEnd(n),i=this._getReboundTransition(this.touchZoom?t:n,r);this.updateViewport(r,i,{isDragging:!1,isPanning:!!i&&this.touchZoom,isZooming:!!i&&this.touchZoom,isRotating:!!i&&this.touchRotate})}return hm._startPinchRotation=null,hm._lastPinchEvent=null,!0}_onDoubleClick(e){if(!this.doubleClickZoom||Date.now()<this._suppressDoubleClickUntil)return!1;let t=this.getCenter(e);if(!this.isPointInBounds(t,e))return!1;let n=this.isFunctionKeyPressed(e),r=this.getZoomPosition(t),i=this.controllerState.zoom({pos:r,scale:n?.5:2});return this.updateViewport(i,this._getTransitionProps({around:r}),{isZooming:!0,isPanning:!0}),this.blockEvents(100),!0}_onDoubleClickDragStart(e){if(!this.doubleClickDragZoom)return this._doubleClickDragAnchor=null,!1;let t=this.getCenter(e);if(!this.isPointInBounds(t,e))return this._doubleClickDragAnchor=null,!1;this._doubleClickDragAnchor=this.getZoomPosition(t);let n=this.controllerState.zoomStart({pos:this._doubleClickDragAnchor},this._getConstraintContext(`zoom`,`start`));return e.scale!==1&&(n=n.zoom({pos:this._doubleClickDragAnchor,scale:e.scale},this._getConstraintContext(`zoom`,`update`))),this.updateViewport(n,lm,{isDragging:!0,isPanning:!0,isZooming:!0}),!0}_onDoubleClickDrag(e){let t=this._doubleClickDragAnchor;if(!t)return!1;let n=this.controllerState.zoom({pos:t,scale:e.scale},this._getConstraintContext(`zoom`,`update`));return this.updateViewport(n,lm,{isDragging:!0,isPanning:!0,isZooming:!0}),!0}_onDoubleClickDragEnd(e){if(!this._doubleClickDragAnchor)return!1;this._doubleClickDragAnchor=null;let t=this.controllerState,n=this._getConstraintContext(`zoom`,`end`),r=t.zoomEnd(n),i=this._getReboundTransition(n,r);return this.updateViewport(r,i,{isDragging:!1,isPanning:!!i,isZooming:!!i}),this._suppressDoubleClickUntil=Date.now()+100,this.blockEvents(100),!0}_onKeyDown(e){if(!this.keyboard)return!1;let t=this.isFunctionKeyPressed(e),{zoomSpeed:n,moveSpeed:r,rotateSpeedX:i,rotateSpeedY:a}=this.keyboard===!0?{}:this.keyboard,{controllerState:o}=this,s,c={};switch(e.srcEvent.code){case`Minus`:s=t?o.zoomOut(n).zoomOut(n):o.zoomOut(n),c.isZooming=!0;break;case`Equal`:s=t?o.zoomIn(n).zoomIn(n):o.zoomIn(n),c.isZooming=!0;break;case`ArrowLeft`:t?(s=o.rotateLeft(i),c.isRotating=!0):(s=o.moveLeft(r),c.isPanning=!0);break;case`ArrowRight`:t?(s=o.rotateRight(i),c.isRotating=!0):(s=o.moveRight(r),c.isPanning=!0);break;case`ArrowUp`:t?(s=o.rotateUp(a),c.isRotating=!0):(s=o.moveUp(r),c.isPanning=!0);break;case`ArrowDown`:t?(s=o.rotateDown(a),c.isRotating=!0):(s=o.moveDown(r),c.isPanning=!0);break;default:return!1}return this.updateViewport(s,this._getTransitionProps(),c),!0}_getTransitionProps(e){let{transition:t}=this;return!t||!t.transitionInterpolator?lm:e?{...t,transitionInterpolator:new cm({...e,...t.transitionInterpolator.opts,makeViewport:this.controllerState.makeViewport})}:t}},_m=Symbol(`constraintAround`),vm=class{constructor(e,t,n,r){this.makeViewport=n,this._viewportProps=this.applyConstraints(e,r),this._state=t}getViewportProps(){return this._viewportProps}getState(){return this._state}};function ym(e,t,n){let r=e-t;return r&&Number.isFinite(r)?t+r*n/(n+Math.abs(r)):t}function bm(e,t,n){let r=Hp(Bp(n?.left??0),e),i=Hp(Bp(n?.right??0),e),a=Hp(Bp(n?.top??0),t),o=Hp(Bp(n?.bottom??0),t);return{x:r,y:a,width:e-r-i,height:t-a-o}}function xm(e,t,n){let[r,i]=e.project(t);return r=Number.isFinite(r)?r:e.width/2,i=Number.isFinite(i)?i:e.height/2,{left:r-n.x,right:n.x+n.width-r,top:i-n.y,bottom:n.y+n.height-i}}var Sm=5,Cm=1.2,wm=512,Tm=[[-1/0,-90],[1/0,90]],Em=1;function Dm([e,t]){if(Math.abs(t)>90&&(t=Math.sign(t)*90),Number.isFinite(e)){let[n,r]=Bd([e,t]);return[n,P(r,0,wm)]}let[,n]=Bd([0,t]);return[e,P(n,0,wm)]}var Om=class extends vm{constructor(e){let{width:t,height:n,latitude:r,longitude:i,zoom:a,bearing:o=0,pitch:s=0,altitude:c=1.5,position:l=[0,0,0],maxZoom:u=20,minZoom:d=0,maxPitch:f=60,minPitch:p=0,startPanLngLat:m,startZoomLngLat:h,startRotatePos:g,startRotateLngLat:_,startBearing:v,startPitch:y,startZoom:b,normalize:x=!0,rubberBand:S=!1}=e,{[_m]:C}=e;H(Number.isFinite(i)),H(Number.isFinite(r)),H(Number.isFinite(a));let w=e.maxBounds||(x?Tm:null),T=e.maxBoundsPadding||null;super({width:t,height:n,latitude:r,longitude:i,zoom:a,bearing:o,pitch:s,altitude:c,maxZoom:u,minZoom:d,maxPitch:f,minPitch:p,normalize:x,position:l,maxBounds:w,maxBoundsPadding:T,rubberBand:S,[_m]:C},{startPanLngLat:m,startZoomLngLat:h,startRotatePos:g,startRotateLngLat:_,startBearing:v,startPitch:y,startZoom:b},e.makeViewport,e.constraintContext),this.getAltitude=e.getAltitude}panStart({pos:e},t){return this._getUpdatedState({startPanLngLat:this._unproject(e)},t)}pan({pos:e,startPos:t},n){let r=this.getState().startPanLngLat||this._unproject(t);if(!r)return this;let i=this.makeViewport(this.getViewportProps()).panByPosition(r,e);return this._getUpdatedState(i,n)}panEnd(e){return this._getUpdatedState({startPanLngLat:null},e)}rotateStart({pos:e}){let t=this.getAltitude?.(e);return this._getUpdatedState({startRotatePos:e,startRotateLngLat:t===void 0?void 0:this._unproject3D(e,t),startBearing:this.getViewportProps().bearing,startPitch:this.getViewportProps().pitch})}rotate({pos:e,deltaAngleX:t=0,deltaAngleY:n=0}){let{startRotatePos:r,startRotateLngLat:i,startBearing:a,startPitch:o}=this.getState();if(!r||a===void 0||o===void 0)return this;let s;if(s=e?this._getNewRotation(e,r,o,a):{bearing:a+t,pitch:o+n},i){let e=this.makeViewport({...this.getViewportProps(),...s}),t=`panByPosition3D`in e?`panByPosition3D`:`panByPosition`;return this._getUpdatedState({...s,...e[t](i,r)})}return this._getUpdatedState(s)}rotateEnd(){return this._getUpdatedState({startRotatePos:null,startRotateLngLat:null,startBearing:null,startPitch:null})}zoomStart({pos:e},t){return this._getUpdatedState({startZoomLngLat:this._unproject(e),startZoom:this.getViewportProps().zoom},t)}zoom({pos:e,startPos:t,scale:n},r){let{startZoom:i,startZoomLngLat:a}=this.getState();return a||=(i=this.getViewportProps().zoom,this._unproject(t)||this._unproject(e)),a?this._getUpdatedState({zoom:i+Math.log2(n),[_m]:{position:a,screenPosition:e}},r):this}zoomEnd(e){return this._getUpdatedState({startZoomLngLat:null,startZoom:null},e)}zoomIn(e=2,t){return this._zoomFromCenter(e,t)}zoomOut(e=2,t){return this._zoomFromCenter(1/e,t)}moveLeft(e=100,t){return this._panFromCenter([e,0],t)}moveRight(e=100,t){return this._panFromCenter([-e,0],t)}moveUp(e=100,t){return this._panFromCenter([0,e],t)}moveDown(e=100,t){return this._panFromCenter([0,-e],t)}rotateLeft(e=15){return this._getUpdatedState({bearing:this.getViewportProps().bearing-e})}rotateRight(e=15){return this._getUpdatedState({bearing:this.getViewportProps().bearing+e})}rotateUp(e=10){return this._getUpdatedState({pitch:this.getViewportProps().pitch+e})}rotateDown(e=10){return this._getUpdatedState({pitch:this.getViewportProps().pitch-e})}shortestPathFrom(e){let t=e.getViewportProps(),n={...this.getViewportProps()},{bearing:r,longitude:i}=n;return Math.abs(r-t.bearing)>180&&(n.bearing=r<0?r+360:r-360),Math.abs(i-t.longitude)>180&&(n.longitude=i<0?i+360:i-360),n}applyConstraints(e,t){let n=e,r=n[_m];delete n[_m];let{maxPitch:i,minPitch:a,pitch:o,bearing:s,normalize:c,maxBounds:l,rubberBand:u}=e;c&&(s<-180||s>180)&&(e.bearing=Xf(s+180,360)-180),e.pitch=P(o,a,i);let d=this._constrainZoom(e.zoom,e),f=u&&t?.mode===`elastic`;if(e.zoom=t?.mode===`preserve`?e.zoom:f?ym(e.zoom,d,Em):d,r){let t=this.makeViewport(e);Object.assign(e,t.panByPosition(r.position,r.screenPosition))}if(c&&(e.longitude<-180||e.longitude>180)&&(e.longitude=Xf(e.longitude+180,360)-180),l){let n=bm(e.width,e.height,e.maxBoundsPadding),r=xm(this.makeViewport({...e,bearing:0,pitch:0}),[e.longitude,e.latitude],n),i=Dm(l[0]),a=Dm(l[1]),o=2**e.zoom,s=[i[0]+r.left/o,i[1]+r.bottom/o],c=[a[0]-r.right/o,a[1]-r.top/o],u=Dm([e.longitude,e.latitude]),d=[P(u[0],s[0],c[0]),P(u[1],s[1],c[1])],p=u.slice();if(n.width>=0&&(p[0]=t?.mode===`preserve`?u[0]:f?ym(u[0],d[0],n.width/2/o):d[0]),n.height>=0&&(p[1]=t?.mode===`preserve`?u[1]:f?ym(u[1],d[1],n.height/2/o):d[1]),p[0]!==u[0]||p[1]!==u[1]){let[t,n]=Vd(p);p[0]!==u[0]&&(e.longitude=t),p[1]!==u[1]&&(e.latitude=n)}}return e}_constrainZoom(e,t){t||=this.getViewportProps();let{maxZoom:n,maxBounds:r}=t,i=r!==null&&t.width>0&&t.height>0,{minZoom:a}=t;if(i){let e=bm(t.width,t.height,t.maxBoundsPadding),i=Dm(r[0]),o=Dm(r[1]),s=o[0]-i[0],c=o[1]-i[1];e.width>0&&Number.isFinite(s)&&s>0&&(a=Math.max(a,Math.log2(e.width/s))),e.height>0&&Number.isFinite(c)&&c>0&&(a=Math.max(a,Math.log2(e.height/c))),a>n&&(a=n)}return P(e,a,n)}_zoomFromCenter(e,t){let{width:n,height:r}=this.getViewportProps();return this.zoom({pos:[n/2,r/2],scale:e},t)}_panFromCenter(e,t){let{width:n,height:r}=this.getViewportProps();return this.pan({startPos:[n/2,r/2],pos:[n/2+e[0],r/2+e[1]]},t)}_getUpdatedState(e,t){return new this.constructor({makeViewport:this.makeViewport,...this.getViewportProps(),...this.getState(),...e,constraintContext:t})}_unproject(e){let t=this.makeViewport(this.getViewportProps());return e&&t.unproject(e)}_unproject3D(e,t){return this.makeViewport(this.getViewportProps()).unproject(e,{targetZ:t})}_getNewRotation(e,t,n,r){let i=e[0]-t[0],a=e[1]-t[1],o=e[1],s=t[1],{width:c,height:l}=this.getViewportProps(),u=i/c,d=0;a>0?Math.abs(l-s)>Sm&&(d=a/(s-l)*Cm):a<0&&s>Sm&&(d=1-o/s),d=P(d,-1,1);let{minPitch:f,maxPitch:p}=this.getViewportProps(),m=r+180*u,h=n;return d>0?h=n+d*(p-n):d<0&&(h=n-d*(f-n)),{pitch:h,bearing:m}}},km=class extends gm{constructor(){super(...arguments),this.ControllerState=Om,this.transition={transitionDuration:300,transitionInterpolator:new cm({transitionProps:{compare:[`longitude`,`latitude`,`zoom`,`bearing`,`pitch`,`position`],required:[`longitude`,`latitude`,`zoom`]}})},this.dragMode=`pan`,this.rotationPivot=`center`,this._getAltitude=e=>{if(this.rotationPivot===`2d`)return 0;if(this.rotationPivot===`3d`&&this.pickPosition){let{x:t,y:n}=this.props,r=this.pickPosition(t+e[0],n+e[1]);if(r&&r.coordinate&&r.coordinate.length>=3)return r.coordinate[2]}}}setProps(e){`rotationPivot`in e&&(this.rotationPivot=e.rotationPivot||`center`),e.getAltitude=this._getAltitude,e.position=e.position||[0,0,0],e.maxBounds=e.maxBounds||(e.normalize===!1?null:Tm),super.setProps(e)}updateViewport(e,t=null,n={}){let r=e.getState();n.isDragging&&r.startRotateLngLat?n={...n,rotationPivotPosition:r.startRotateLngLat}:n.isDragging===!1&&(n={...n,rotationPivotPosition:void 0}),super.updateViewport(e,t,n)}},Am=class extends Xp{constructor(e={}){super(e)}getViewportType(){return dp}get ControllerType(){return km}};Am.displayName=`MapView`;var jm=new qf;function Mm(e,t){return(e.order??1/0)-(t.order??1/0)}var Nm=class{constructor(e){this._resolvedEffects=[],this._defaultEffects=[],this.effects=[],this._context=e,this._needsRedraw=`Initial render`,this._setEffects([])}addDefaultEffect(e){let t=this._defaultEffects;if(!t.find(t=>t.id===e.id)){let n=t.findIndex(t=>Mm(t,e)>0);n<0?t.push(e):t.splice(n,0,e),e.setup(this._context),this._setEffects(this.effects)}}setProps(e){`effects`in e&&(Ip(e.effects,this.effects,1)||this._setEffects(e.effects))}needsRedraw(e={clearRedrawFlags:!1}){let t=this._needsRedraw;return e.clearRedrawFlags&&(this._needsRedraw=!1),t}getEffects(){return this._resolvedEffects}_setEffects(e){let t={};for(let e of this.effects)t[e.id]=e;let n=[];for(let r of e){let e=t[r.id],i=r;e&&e!==r?e.setProps?(e.setProps(r.props),i=e):e.cleanup(this._context):e||r.setup(this._context),n.push(i),delete t[r.id]}for(let e in t)t[e].cleanup(this._context);this.effects=n,this._resolvedEffects=n.concat(this._defaultEffects),e.some(e=>e instanceof qf)||this._resolvedEffects.push(jm),this._needsRedraw=`effects changed`}finalize(){for(let e of this._resolvedEffects)e.cleanup(this._context);this.effects.length=0,this._resolvedEffects.length=0,this._defaultEffects.length=0}},Pm=class extends zf{shouldDrawLayer(e){let{operation:t}=e.props;return t.includes(`draw`)||t.includes(`terrain`)}render(e){return this._render(e)}},Fm=`deckRenderer.renderLayers`,Im=class{constructor(e,t={}){this.device=e,this.stats=t.stats,this.layerFilter=null,this.drawPickingColors=!1,this.drawLayersPass=new Pm(e),this.pickLayersPass=new vp(e),this.renderCount=0,this._needsRedraw=`Initial render`,this.renderBuffers=[],this.lastPostProcessEffect=null}setProps(e){this.layerFilter!==e.layerFilter&&(this.layerFilter=e.layerFilter,this._needsRedraw=`layerFilter changed`),this.drawPickingColors!==e.drawPickingColors&&(this.drawPickingColors=e.drawPickingColors,this._needsRedraw=`drawPickingColors changed`)}renderLayers(e){let t=this.drawPickingColors?this.pickLayersPass:this.drawLayersPass,n={layerFilter:this.layerFilter,isPicking:this.drawPickingColors,...e};if(!e.viewports.length){let e=t.render(n),r=`stats`in e?e.stats:e;this._updateStats(r);return}n.effects&&this._preRender(n.effects,n);let r=this.lastPostProcessEffect?this.renderBuffers[0]:n.target;this.lastPostProcessEffect&&(n.clearColor=[0,0,0,0],n.clearCanvas=!0);let i=t.render({...n,target:r}),a=`stats`in i?i.stats:i;n.effects&&(this.lastPostProcessEffect&&(n.clearCanvas=e.clearCanvas===void 0||e.clearCanvas),this._postRender(n.effects,n)),this.renderCount++,el(Fm,this,a,e),this._updateStats(a)}needsRedraw(e={clearRedrawFlags:!1}){let t=this._needsRedraw;return e.clearRedrawFlags&&(this._needsRedraw=!1),t}finalize(){let{renderBuffers:e}=this;for(let t of e)t.delete();e.length=0}_updateStats(e){if(!this.stats)return;let t=0;for(let{visibleCount:n}of e)t+=n;this.stats.get(`Layers rendered`).addCount(t)}_preRender(e,t){this.lastPostProcessEffect=null,t.preRenderStats=t.preRenderStats||{};for(let n of e)t.preRenderStats[n.id]=n.preRender(t),n.postRender&&(this.lastPostProcessEffect=n.id);this.lastPostProcessEffect&&this._resizeRenderBuffers(t.canvasContext)}_resizeRenderBuffers(e=this.device.canvasContext){let{renderBuffers:t}=this,n=e.getDrawingBufferSize(),[r,i]=n;t.length===0&&[0,1].map(e=>{let n=this.device.createTexture({sampler:{minFilter:`linear`,magFilter:`linear`},width:r,height:i});t.push(this.device.createFramebuffer({id:`deck-renderbuffer-${e}`,colorAttachments:[n]}))});for(let e of t)e.resize(n)}_postRender(e,t){let{renderBuffers:n}=this,r=t.target??t.canvasContext?.getCurrentFramebuffer()??t.target,i={...t,inputBuffer:n[0],swapBuffer:n[1]};for(let t of e)if(t.postRender){i.target=t.id===this.lastPostProcessEffect?r:void 0;let e=t.postRender(i);i.inputBuffer=e,i.swapBuffer=e===n[0]?n[1]:n[0]}}},Lm={pickedColor:null,pickedObjectIndex:-1};function Rm({pickedColors:e,decodePickingColor:t,deviceX:n,deviceY:r,deviceRadius:i,deviceRect:a}){let{x:o,y:s,width:c,height:l}=a,u=i*i,d=-1,f=0;for(let t=0;t<l;t++){let i=t+s-r,a=i*i;if(a>u)f+=4*c;else for(let t=0;t<c;t++){if(e[f+3]-1>=0){let e=t+o-n,r=e*e+a;r<=u&&(u=r,d=f)}f+=4}}if(d>=0){let n=e.slice(d,d+4),r=t(n);if(r){let e=Math.floor(d/4/c),t=d/4-e*c;return{...r,pickedColor:n,pickedX:o+t,pickedY:s+e}}z.error(`Picked non-existent layer. Is picking buffer corrupt?`)()}return Lm}function zm({pickedColors:e,decodePickingColor:t}){let n=new Map;if(e){for(let r=0;r<e.length;r+=4)if(e[r+3]-1>=0){let i=e.slice(r,r+4),a=i.join(`,`);if(!n.has(a)){let e=t(i);e?n.set(a,{...e,color:i}):z.error(`Picked non-existent layer. Is picking buffer corrupt?`)()}}}return Array.from(n.values())}function Bm({pickInfo:e,viewports:t,pixelRatio:n,x:r,y:i,z:a}){let o=t[0];t.length>1&&(o=Um(e?.pickedViewports||t,{x:r,y:i}));let s;if(o){let e=[r-o.x,i-o.y];a!==void 0&&(e[2]=a),s=o.unproject(e)}return{color:null,layer:null,viewport:o,index:-1,picked:!1,x:r,y:i,pixel:[r,i],coordinate:s,devicePixel:e&&`pickedX`in e?[e.pickedX,e.pickedY]:void 0,pixelRatio:n}}function Vm(e){let{pickInfo:t,lastPickedInfo:n,mode:r,layers:i}=e,{pickedColor:a,pickedLayer:o,pickedObjectIndex:s}=t,c=o?[o]:[];if(r===`hover`){let e=n.index,t=n.layerId,r=o?o.props.id:null;if(r!==t||s!==e){if(r!==t){let e=i.find(e=>e.props.id===t);e&&c.unshift(e)}n.layerId=r,n.index=s,n.info=null}}let l=Bm(e),u=new Map;return u.set(null,l),c.forEach(e=>{let t={...l};e===o&&(t.color=a,t.index=s,t.picked=!0),t=Hm({layer:e,info:t,mode:r});let i=t.layer;e===o&&r===`hover`&&(n.info=t),u.set(i.id,t),r===`hover`&&i.updateAutoHighlight(t)}),u}function Hm({layer:e,info:t,mode:n}){for(;e&&t;){let r=t.layer||null;t.sourceLayer=r,t.layer=e,t=e.getPickingInfo({info:t,mode:n,sourceLayer:r}),e=e.parent}return t}function Um(e,t){for(let n=e.length-1;n>=0;n--){let r=e[n];if(r.containsPixel(t))return r}return e[0]}var Wm=class{constructor(e,t={}){this._pickable=!0,this.device=e,this.stats=t.stats,this.pickLayersPass=new vp(e),this.lastPickedInfo={index:-1,layerId:null,info:null}}setProps(e){`layerFilter`in e&&(this.layerFilter=e.layerFilter),`_pickable`in e&&(this._pickable=e._pickable)}finalize(){this.pickingFBO&&this.pickingFBO.destroy(),this.depthFBO&&this.depthFBO.destroy()}pickObjectAsync(e){return this._pickClosestObjectAsync(e)}pickObjectsAsync(e){return this._pickVisibleObjectsAsync(e)}pickObject(e){return this._pickClosestObject(e)}pickObjects(e){return this._pickVisibleObjects(e)}getLastPickedObject({x:e,y:t,layers:n,viewports:r},i=this.lastPickedInfo.info){let a=i&&i.layer&&i.layer.id,o=i&&i.viewport&&i.viewport.id,s=a?n.find(e=>e.id===a):null,c=o&&r.find(e=>e.id===o)||r[0],l={x:e,y:t,viewport:c,coordinate:c&&c.unproject([e-c.x,t-c.y]),layer:s};return{...i,...l}}_resizeBuffer(e=this.device.getDefaultCanvasContext()){if(!this.pickingFBO){let e=this.device.createTexture({format:`rgba8unorm`,width:1,height:1,usage:M.RENDER_ATTACHMENT|M.COPY_SRC});if(this.pickingFBO=this.device.createFramebuffer({colorAttachments:[e],depthStencilAttachment:`depth16unorm`}),this.device.isTextureFormatRenderable(`rgba32float`)){let e=this.device.createTexture({format:`rgba32float`,width:1,height:1,usage:M.RENDER_ATTACHMENT|M.COPY_SRC}),t=this.device.createFramebuffer({colorAttachments:[e],depthStencilAttachment:`depth16unorm`});this.depthFBO=t}}let[t,n]=e.getDrawingBufferSize();this.pickingFBO?.resize({width:t,height:n}),this.depthFBO?.resize({width:t,height:n})}_getPickable(e){if(this._pickable===!1)return null;let t=e.filter(e=>this.pickLayersPass.shouldDrawLayer(e)&&!e.isComposite);return t.length?t:null}async _pickClosestObjectAsync({layers:e,views:t,viewports:n,x:r,y:i,radius:a=0,depth:o=1,mode:s=`query`,unproject3D:c,canvasContext:l=this.device.getDefaultCanvasContext(),onViewportActive:u,effects:d}){let f=l.cssToDeviceRatio(),p=this._getPickable(e);if(!p||n.length===0)return{result:[],emptyInfo:Bm({viewports:n,x:r,y:i,pixelRatio:f})};this._resizeBuffer(l);let m=l.cssToDevicePixels([r,i],!0),h=[m.x+Math.floor(m.width/2),m.y+Math.floor(m.height/2)],g=Math.round(a*f),{width:_,height:v}=this.pickingFBO,y=this._getPickingRect({deviceX:h[0],deviceY:h[1],deviceRadius:g,deviceWidth:_,deviceHeight:v}),b={x:r-a,y:i-a,width:a*2+1,height:a*2+1},x,S=[],C=new Set;for(let e=0;e<o;e++){let a;a=y?Rm({...await this._drawAndSampleAsync({layers:p,views:t,viewports:n,onViewportActive:u,deviceRect:y,cullRect:b,effects:d,pass:`picking:${s}`,canvasContext:l}),deviceX:h[0],deviceY:h[1],deviceRadius:g,deviceRect:y}):{pickedColor:null,pickedObjectIndex:-1};let m,_=this._getDepthLayers(a,p,c);if(_.length>0){let{pickedColors:e}=await this._drawAndSampleAsync({layers:_,views:t,viewports:n,onViewportActive:u,deviceRect:{x:a.pickedX??h[0],y:a.pickedY??h[1],width:1,height:1},cullRect:b,effects:d,pass:`picking:${s}:z`,canvasContext:l},!0);e[3]&&(m=e[0])}a.pickedLayer&&e+1<o&&(C.add(a.pickedLayer),a.pickedLayer.disablePickingIndex(a.pickedObjectIndex)),x=Vm({pickInfo:a,lastPickedInfo:this.lastPickedInfo,mode:s,layers:p,viewports:n,x:r,y:i,z:m,pixelRatio:f});for(let e of x.values())e.layer&&S.push(e);if(!a.pickedColor)break}for(let e of C)e.restorePickingColors();return{result:S,emptyInfo:x.get(null)}}_pickClosestObject({layers:e,views:t,viewports:n,x:r,y:i,radius:a=0,depth:o=1,mode:s=`query`,unproject3D:c,canvasContext:l=this.device.getDefaultCanvasContext(),onViewportActive:u,effects:d}){let f=l.cssToDeviceRatio(),p=this._getPickable(e);if(!p||n.length===0)return{result:[],emptyInfo:Bm({viewports:n,x:r,y:i,pixelRatio:f})};this._resizeBuffer(l);let m=l.cssToDevicePixels([r,i],!0),h=[m.x+Math.floor(m.width/2),m.y+Math.floor(m.height/2)],g=Math.round(a*f),{width:_,height:v}=this.pickingFBO,y=this._getPickingRect({deviceX:h[0],deviceY:h[1],deviceRadius:g,deviceWidth:_,deviceHeight:v}),b={x:r-a,y:i-a,width:a*2+1,height:a*2+1},x,S=[],C=new Set;for(let e=0;e<o;e++){let a;a=y?Rm({...this._drawAndSample({layers:p,views:t,viewports:n,onViewportActive:u,deviceRect:y,cullRect:b,effects:d,pass:`picking:${s}`,canvasContext:l}),deviceX:h[0],deviceY:h[1],deviceRadius:g,deviceRect:y}):{pickedColor:null,pickedObjectIndex:-1};let m,_=this._getDepthLayers(a,p,c);if(_.length>0){let{pickedColors:e}=this._drawAndSample({layers:_,views:t,viewports:n,onViewportActive:u,deviceRect:{x:a.pickedX??h[0],y:a.pickedY??h[1],width:1,height:1},cullRect:b,effects:d,pass:`picking:${s}:z`,canvasContext:l},!0);e[3]&&(m=e[0])}a.pickedLayer&&e+1<o&&(C.add(a.pickedLayer),a.pickedLayer.disablePickingIndex(a.pickedObjectIndex)),x=Vm({pickInfo:a,lastPickedInfo:this.lastPickedInfo,mode:s,layers:p,viewports:n,x:r,y:i,z:m,pixelRatio:f});for(let e of x.values())e.layer&&S.push(e);if(!a.pickedColor)break}for(let e of C)e.restorePickingColors();return{result:S,emptyInfo:x.get(null)}}async _pickVisibleObjectsAsync({layers:e,views:t,viewports:n,x:r,y:i,width:a=1,height:o=1,mode:s=`query`,maxObjects:c=null,canvasContext:l=this.device.getDefaultCanvasContext(),onViewportActive:u,effects:d}){let f=this._getPickable(e);if(!f||n.length===0)return[];this._resizeBuffer(l);let p=l.cssToDeviceRatio(),m=l.cssToDevicePixels([r,i],!0),h=m.x,g=m.y+m.height,_=l.cssToDevicePixels([r+a,i+o],!0),v=_.x+_.width,y=_.y,b={x:h,y,width:v-h,height:g-y},x=zm(await this._drawAndSampleAsync({layers:f,views:t,viewports:n,onViewportActive:u,deviceRect:b,cullRect:{x:r,y:i,width:a,height:o},effects:d,pass:`picking:${s}`,canvasContext:l})),S=new Map,C=[],w=Number.isFinite(c);for(let e=0;e<x.length&&!(w&&C.length>=c);e++){let t=x[e],n={color:t.pickedColor,layer:null,index:t.pickedObjectIndex,picked:!0,x:r,y:i,pixelRatio:p};n=Hm({layer:t.pickedLayer,info:n,mode:s});let a=n.layer.id;S.has(a)||S.set(a,new Set);let o=S.get(a),c=n.object??n.index;o.has(c)||(o.add(c),C.push(n))}return C}_pickVisibleObjects({layers:e,views:t,viewports:n,x:r,y:i,width:a=1,height:o=1,mode:s=`query`,maxObjects:c=null,canvasContext:l=this.device.getDefaultCanvasContext(),onViewportActive:u,effects:d}){let f=this._getPickable(e);if(!f||n.length===0)return[];this._resizeBuffer(l);let p=l.cssToDeviceRatio(),m=l.cssToDevicePixels([r,i],!0),h=m.x,g=m.y+m.height,_=l.cssToDevicePixels([r+a,i+o],!0),v=_.x+_.width,y=_.y,b={x:h,y,width:v-h,height:g-y},x=zm(this._drawAndSample({layers:f,views:t,viewports:n,onViewportActive:u,deviceRect:b,cullRect:{x:r,y:i,width:a,height:o},effects:d,pass:`picking:${s}`,canvasContext:l})),S=new Map,C=[],w=Number.isFinite(c);for(let e=0;e<x.length&&!(w&&C.length>=c);e++){let t=x[e],n={color:t.pickedColor,layer:null,index:t.pickedObjectIndex,picked:!0,x:r,y:i,pixelRatio:p};n=Hm({layer:t.pickedLayer,info:n,mode:s});let a=n.layer.id;S.has(a)||S.set(a,new Set);let o=S.get(a),c=n.object??n.index;o.has(c)||(o.add(c),C.push(n))}return C}async _drawAndSampleAsync({layers:e,views:t,viewports:n,onViewportActive:r,deviceRect:i,cullRect:a,effects:o,pass:s,canvasContext:c},l=!1){let u=l?this.depthFBO:this.pickingFBO,d={layers:e,layerFilter:this.layerFilter,views:t,viewports:n,onViewportActive:r,pickingFBO:u,deviceRect:i,cullRect:a,effects:o,pass:s,canvasContext:c,pickZ:l,preRenderStats:{},isPicking:!0};for(let e of o)e.useInPicking&&(d.preRenderStats[e.id]=e.preRender(d));let{decodePickingColor:f,stats:p}=this.pickLayersPass.render(d);this._updateStats(p);let{x:m,y:h,width:g,height:_}=i,v=u.colorAttachments[0]?.texture;if(!v)throw Error(`Picking framebuffer color attachment is missing`);let y=await this._readTextureDataAsync(v,{x:m,y:h,width:g,height:_},l?Float32Array:Uint8Array);if(!l){let e=!1;for(let t=3;t<y.length;t+=4)if(y[t]!==0){e=!0;break}!e&&y.length>0&&z.warn(`Async pick readback returned only zero alpha values`,{deviceRect:i,bytes:Array.from(y.subarray(0,Math.min(y.length,16)))})()}return{pickedColors:y,decodePickingColor:f}}async _readTextureDataAsync(e,t,n){let{width:r,height:i}=t,a=e.computeMemoryLayout(t),o=this.device.createBuffer({byteLength:a.byteLength,usage:C.COPY_DST|C.MAP_READ});try{e.readBuffer(t,o);let s=await o.readAsync(0,a.byteLength),c=n.BYTES_PER_ELEMENT;if(a.bytesPerRow%c!==0)throw Error(`Texture readback row stride ${a.bytesPerRow} is not aligned to ${c}-byte elements.`);let l=new n(s.buffer,s.byteOffset,a.byteLength/c),u=r*4,d=a.bytesPerRow/c;if(d<u)throw Error(`Texture readback row stride ${d} is smaller than packed row length ${u}.`);let f=new n(r*i*4);for(let e=0;e<i;e++){let t=e*d;f.set(l.subarray(t,t+u),e*u)}return f}finally{o.destroy()}}_drawAndSample({layers:e,views:t,viewports:n,onViewportActive:r,deviceRect:i,cullRect:a,effects:o,pass:s,canvasContext:c},l=!1){let u=l?this.depthFBO:this.pickingFBO,d={layers:e,layerFilter:this.layerFilter,views:t,viewports:n,onViewportActive:r,pickingFBO:u,deviceRect:i,cullRect:a,effects:o,pass:s,canvasContext:c,pickZ:l,preRenderStats:{},isPicking:!0};for(let e of o)e.useInPicking&&(d.preRenderStats[e.id]=e.preRender(d));let{decodePickingColor:f,stats:p}=this.pickLayersPass.render(d);this._updateStats(p);let{x:m,y:h,width:g,height:_}=i,v=new(l?Float32Array:Uint8Array)(g*_*4);return this.device.readPixelsToArrayWebGL(u,{sourceX:m,sourceY:h,sourceWidth:g,sourceHeight:_,target:v}),{pickedColors:v,decodePickingColor:f}}_updateStats(e){if(!this.stats)return;let t=0;for(let{visibleCount:n}of e)t+=n;this.stats.get(`Layers picked`).addCount(t)}_getDepthLayers(e,t,n){if(!n||!this.depthFBO)return[];let{pickedLayer:r}=e,i=r?.state?.terrainDrawMode===`drape`;return r&&!i?[r]:t.filter(e=>e.props.operation.includes(`terrain`))}_getPickingRect({deviceX:e,deviceY:t,deviceRadius:n,deviceWidth:r,deviceHeight:i}){let a=Math.max(0,e-n),o=Math.max(0,t-n),s=Math.min(r,e+n+1)-a,c=Math.min(i,t+n+1)-o;return s<=0||c<=0?null:{x:a,y:o,width:s,height:c}}},Gm={"top-left":{top:0,left:0},"top-right":{top:0,right:0},"bottom-left":{bottom:0,left:0},"bottom-right":{bottom:0,right:0},fill:{top:0,left:0,bottom:0,right:0}},Km=`top-left`,qm=`root`,Jm=class{constructor({deck:e,parentElement:t}){this.defaultWidgets=[],this.widgets=[],this.resolvedWidgets=[],this.containers={},this.lastViewports={},this.deck=e,t?.classList.add(`deck-widget-container`),this.parentElement=t}getWidgets(){return this.resolvedWidgets}setProps(e){if(e.widgets&&!Ip(e.widgets,this.widgets,1)){let t=e.widgets.filter(Boolean);this._setWidgets(t)}}finalize(){for(let e of this.getWidgets())this._removeWidget(e);this.defaultWidgets.length=0,this.resolvedWidgets.length=0;for(let e in this.containers)this.containers[e].remove()}addDefault(e){this.defaultWidgets.find(t=>t.id===e.id)||(this._addWidget(e),this.defaultWidgets.push(e),this._setWidgets(this.widgets))}onRedraw({viewports:e,layers:t}){let n=e.reduce((e,t)=>(e[t.id]=t,e),{});for(let r of this.getWidgets()){let{viewId:i}=r;if(i){let e=n[i];e&&(r.onViewportChange&&r.onViewportChange(e),r.onRedraw?.({viewports:[e],layers:t}))}else{if(r.onViewportChange)for(let t of e)r.onViewportChange(t);r.onRedraw?.({viewports:e,layers:t})}}this.lastViewports=n,this._updateContainers()}onHover(e,t){for(let n of this.getWidgets()){let{viewId:r}=n;(!r||r===e.viewport?.id)&&n.onHover?.(e,t)}}getCanvasBounds(e){let t=(this.deck?.getCanvas?.())?.getBoundingClientRect(),n=this.parentElement?.getBoundingClientRect(),r=this.deck?.getCanvasContext?.(e?.id);if(r&&n){r.updatePosition();let[e,t]=r.getPosition(),[i,a]=r.getCSSSize();return{x:e-n.left,y:t-n.top,width:i,height:a}}return{x:t&&n?t.left-n.left:0,y:t&&n?t.top-n.top:0,width:t?.width||this.deck?.width||0,height:t?.height||this.deck?.height||0}}onEvent(e,t){let n=rd[t.type];if(n)for(let r of this.getWidgets()){let{viewId:i}=r;(!i||i===e.viewport?.id)&&r[n]?.(e,t)}}_setWidgets(e){let t={};for(let e of this.resolvedWidgets)t[e.id]=e;this.resolvedWidgets.length=0;for(let e of this.defaultWidgets)t[e.id]=null,this.resolvedWidgets.push(e);for(let n of e){let e=t[n.id];e?e.viewId!==n.viewId||e.placement!==n.placement?(this._removeWidget(e),this._addWidget(n)):n!==e&&(e.setProps(n.props),n=e):this._addWidget(n),t[n.id]=null,this.resolvedWidgets.push(n)}for(let e in t){let n=t[e];n&&this._removeWidget(n)}this.widgets=e}_addWidget(e){let{viewId:t=null,placement:n=Km}=e,r=e.props._container??t;e.widgetManager=this,e.deck=this.deck,e.rootElement=e._onAdd({deck:this.deck,viewId:t}),e.rootElement&&this._getContainer(r,n).append(e.rootElement),e.updateHTML()}_removeWidget(e){e.onRemove?.(),e.rootElement&&e.rootElement.remove(),e.rootElement=void 0,e.deck=void 0,e.widgetManager=void 0}_getContainer(e,t){if(e&&typeof e!=`string`)return e;let n=e||qm,r=this.containers[n];r||(r=document.createElement(`div`),r.style.pointerEvents=`none`,r.style.position=`absolute`,r.style.overflow=`hidden`,this.parentElement?.append(r),this.containers[n]=r);let i=r.querySelector(`.${t}`);return i||(i=globalThis.document.createElement(`div`),i.className=t,i.style.position=`absolute`,i.style.zIndex=`2`,Object.assign(i.style,Gm[t]),r.append(i)),i}_updateContainers(){for(let e in this.containers){let t=this.lastViewports[e]||null,n=e===qm||t,r=this.containers[e];if(n){let e=this._getContainerBounds(t);r.style.display=`block`,r.style.left=`${e.x}px`,r.style.top=`${e.y}px`,r.style.width=`${e.width}px`,r.style.height=`${e.height}px`}else r.style.display=`none`}}_getContainerBounds(e){if(!e)return{x:0,y:0,width:this.parentElement?.clientWidth||this.deck.width,height:this.parentElement?.clientHeight||this.deck.height};let t=this.getCanvasBounds(e);return{x:t.x+e.x,y:t.y+e.y,width:e.width,height:e.height}}};function Ym(e,t){t&&Object.entries(t).map(([t,n])=>{t.startsWith(`--`)?e.style.setProperty(t,n):e.style[t]=n})}function Xm(e,t){t&&Object.keys(t).map(t=>{t.startsWith(`--`)?e.style.removeProperty(t):e.style[t]=``})}var Zm=class{constructor(e){this.viewId=null,this.props={...this.constructor.defaultProps,...e},this.id=this.props.id}setProps(e){let t=this.props,n=this.rootElement;n&&t.className!==e.className&&(t.className&&n.classList.remove(t.className),e.className&&n.classList.add(e.className)),n&&!Ip(t.style,e.style,1)&&(Xm(n,t.style),Ym(n,e.style)),Object.assign(this.props,e),this.updateHTML()}updateHTML(){this.rootElement&&this.onRenderHTML(this.rootElement)}get viewIds(){return this.viewId?[this.viewId]:this.deck?.getViews().map(e=>e.id)??[]}getViewState(e){return this.deck?.viewManager?.getViewState(e)||{}}setViewState(e,t){this.deck?._onViewStateChange({viewId:e,viewState:t,interactionState:{}})}onCreateRootElement(){let e=[`deck-widget`,this.className,this.props.className],t=document.createElement(`div`);return e.filter(e=>typeof e==`string`&&e.length>0).forEach(e=>t.classList.add(e)),Ym(t,this.props.style),t}_onAdd(e){return this.onAdd(e)??this.onCreateRootElement()}onAdd(e){}onRemove(){}onViewportChange(e){}onRedraw(e){}onHover(e,t){}onClick(e,t){}onDrag(e,t){}onDragStart(e,t){}onDragEnd(e,t){}};Zm.defaultProps={id:`widget`,style:{},_container:null,className:``};var Qm={zIndex:`1`,position:`absolute`,pointerEvents:`none`,color:`#a0a7b4`,backgroundColor:`#29323c`,padding:`10px`,top:`0`,left:`0`,display:`none`},$m=class extends Zm{constructor(e={}){super(e),this.id=`default-tooltip`,this.placement=`fill`,this.className=`deck-tooltip`,this.isVisible=!1,this.setProps(e)}onCreateRootElement(){let e=document.createElement(`div`);return e.className=this.className,Object.assign(e.style,Qm),e}onRenderHTML(e){}onViewportChange(e){this.isVisible&&e.id===this.lastViewport?.id&&!e.equals(this.lastViewport)&&this.setTooltip(null),this.lastViewport=e}onHover(e){let{deck:t}=this,n=t&&t.props.getTooltip;if(!n)return;let r=n(e),i=this.widgetManager?.getCanvasBounds(e.viewport),a=e.x+(i?.x||0),o=e.y+(i?.y||0);this.setTooltip(r,a,o)}setTooltip(e,t,n){let r=this.rootElement;if(r){if(typeof e==`string`)r.innerText=e;else if(e)e.text&&(r.innerText=e.text),e.html&&(r.innerHTML=e.html),e.className&&(r.className=e.className);else{this.isVisible=!1,r.style.display=`none`;return}this.isVisible=!0,r.style.display=`block`,r.style.transform=`translate(${t}px, ${n}px)`,e&&typeof e==`object`&&`style`in e&&Object.assign(r.style,e.style)}}};$m.defaultProps={...Zm.defaultProps};var eh=class{constructor(e){this.targets={},this.order=[],this.eventManagers={},this._eventRootToCanvasId=new WeakMap,this._createEventManager=e.createEventManager,this._getEventRoot=e.getEventRoot}finalize(){for(let e of Object.values(this.targets))e.eventManager.destroy(),e.presentationContext.destroy();this.targets={},this.order=[],this.eventManagers={},this._eventRootToCanvasId=new WeakMap}syncCanvasEntries(e){let t=this._normalizeCanvasList(e.canvases),n={},r=[],i=new Map;for(let{canvas:e}of t){let t=this._getEventRoot(e);i.set(t,(i.get(t)||0)+1)}for(let{id:a,canvas:o}of t){let t=this._getEventRoot(o),s=i.get(t)===1?t:o,c=this.targets[a];if(!c||c.device!==e.device||c.canvas!==o||c.eventRoot!==s){c?.eventManager.destroy(),c?.presentationContext.destroy();let t=e.device.createPresentationContext({id:a,canvas:o,useDevicePixels:e.useDevicePixels,autoResize:!0});c={id:a,device:e.device,canvas:o,eventRoot:s,presentationContext:t,eventManager:this._createEventManager(s)}}this._eventRootToCanvasId.set(s,a),this._eventRootToCanvasId.set(o,a),n[a]=c,r.push(a)}for(let[e,t]of Object.entries(this.targets))n[e]||(t.eventManager.destroy(),t.presentationContext.destroy());this.targets=n,this.order=r;let a=Object.fromEntries(Object.entries(n).map(([e,t])=>[e,t.eventManager]));this._haveSameEventManagers(a)||(this.eventManagers=a)}getCanvasIdFromEvent(e){return e?this._eventRootToCanvasId.get(e):void 0}getTarget(e){return this.targets[e||this.order[0]||`default-canvas`]||null}_normalizeCanvasList(e=[]){let t=new Set;return e.map((e,n)=>{let r,i;return typeof e==`string`?(r=document.getElementById(e),H(r,`Canvas with id ${e} not found`),i=e):(r=e,i=r.id||`deckgl-canvas-${n}`),H(!t.has(i),`Duplicate canvas id ${i}`),t.add(i),{id:i,canvas:r}})}_haveSameEventManagers(e){let t=Object.keys(e),n=Object.keys(this.eventManagers);return t.length===n.length&&t.every(t=>e[t]===this.eventManagers[t])}};function th(){}var nh={id:``,width:`100%`,height:`100%`,style:null,viewState:null,initialViewState:null,pickingRadius:0,pickAsync:`auto`,layerFilter:null,parameters:{},parent:null,device:null,deviceProps:{},gl:null,canvas:null,_canvases:null,layers:[],effects:[],views:null,controller:null,useDevicePixels:!0,touchAction:`none`,eventRecognizerOptions:{},_framebuffer:null,_animate:!1,_pickable:!0,_typedArrayManagerProps:{},_customRender:null,widgets:[],onDeviceInitialized:th,onWebGLInitialized:th,onResize:th,onViewStateChange:th,onInteractionStateChange:th,onBeforeRender:th,onAfterRender:th,onLoad:th,onError:e=>z.error(e.message,e.cause)(),onHover:null,onClick:null,onDragStart:null,onDrag:null,onDragEnd:null,_onMetrics:null,getCursor:({isDragging:e})=>e?`grabbing`:`grab`,getTooltip:null,debug:!1,drawPickingColors:!1},rh=class{constructor(e){this.width=0,this.height=0,this.userData={},this.device=null,this.canvas=null,this.viewManager=null,this.layerManager=null,this.effectManager=null,this.deckRenderer=null,this.deckPicker=null,this.eventManager=null,this.eventManagers={},this.widgetManager=null,this.tooltip=null,this.animationLoop=null,this._canvasContext=null,this._deviceResizeHandler=null,this.cursorState={isHovering:!1,isDragging:!1},this.stats=new j({id:`deck.gl`}),this.metrics={fps:0,setPropsTime:0,layersCount:0,drawLayersCount:0,updateLayersCount:0,updateAttributesCount:0,updateAttributesTime:0,framesRedrawn:0,pickTime:0,pickCount:0,pickLayersCount:0,gpuTime:0,gpuTimePerFrame:0,cpuTime:0,cpuTimePerFrame:0,bufferMemory:0,textureMemory:0,renderbufferMemory:0,gpuMemory:0},this._metricsCounter=0,this._hoverPickSequence=0,this._pointerDownPickSequence=0,this._needsRedraw=`Initial render`,this._canvasManager=new eh({createEventManager:e=>this._createEventManager(e),getEventRoot:e=>this._getEventRoot(e)}),this._ownedCanvas=null,this._pickRequest={mode:`hover`,x:-1,y:-1,radius:0,canvasId:void 0,event:null,unproject3D:!1},this._lastPointerDownInfo=null,this._lastPointerDownInfoPromise=null,this._onPointerMove=e=>{let{_pickRequest:t}=this,n=this._getCanvasIdFromEvent(e);if(e.type===`pointerleave`)t.x=-1,t.y=-1,t.radius=0,t.canvasId=n;else if(e.leftButton||e.rightButton)return;else{let r=e.offsetCenter;if(!r)return;t.x=r.x,t.y=r.y,t.radius=this.props.pickingRadius,t.canvasId=n}this.layerManager&&(this.layerManager.context.mousePosition={x:t.x,y:t.y}),t.event=e},this._onEvent=e=>{let t=rd[e.type],n=e.offsetCenter,r=this._getCanvasIdFromEvent(e);if(!t||!n||!this.layerManager)return;let i=this.layerManager.getLayers(),a=this._getInternalPickingMode();if(a){if(a===`sync`){let t=e.type===`click`&&this._shouldUnproject3D(i)?this._getFirstPickedInfo(this._pickPointSync(this._getPointPickOptions(n.x,n.y,{unproject3D:!0,canvasId:r},i))):this._getLastPointerDownPickingInfo(n.x,n.y,r,i);this._dispatchPickingEvent(t,e);return}(this._lastPointerDownInfoPromise||Promise.resolve(this._getLastPointerDownPickingInfo(n.x,n.y,r,i))).then(t=>{this._dispatchPickingEvent(t,e)}).catch(e=>this.props.onError?.(e))}},this._onPointerDown=e=>{let t=e.offsetCenter,n=this._getCanvasIdFromEvent(e);if(!t)return;let r=this._getInternalPickingMode();if(!r)return;let i=this.layerManager?.getLayers()||[],a=++this._pointerDownPickSequence;if(r===`sync`){let e=this._pickPointSync({x:t.x,y:t.y,canvasId:n,radius:this.props.pickingRadius}),r=this._getFirstPickedInfo(e);this._lastPointerDownInfo=r,this._lastPointerDownInfoPromise=Promise.resolve(r);return}let o=this._pickPointAsync(this._getPointPickOptions(t.x,t.y,{canvasId:n},i)).then(e=>this._getFirstPickedInfo(e)).then(e=>(a===this._pointerDownPickSequence&&(this._lastPointerDownInfo=e),e)).catch(e=>{this.props.onError?.(e);let r=this.deckPicker&&this.viewManager?this._getLastPointerDownPickingInfo(t.x,t.y,n,i):{};return a===this._pointerDownPickSequence&&(this._lastPointerDownInfo=r),r});this._lastPointerDownInfo=null,this._lastPointerDownInfoPromise=o};let t=e;this.props={...nh,...e},e=this.props,this._validateCanvasConfiguration(e),e.viewState&&e.initialViewState&&z.warn("View state tracking is disabled. Use either `initialViewState` for auto update or `viewState` for manual update.")(),this.viewState=this.props.initialViewState,e.device&&(this.device=e.device,this._setDeviceCanvasContext(e.device));let n=this.device;!n&&e.gl&&(e.gl instanceof WebGLRenderingContext&&z.error(`WebGL1 context not supported.`)(),n=ya.attach(e.gl,{_cacheShaders:!0,_cachePipelines:!0,...this.props.deviceProps})),n||=this._createDevice(e),this.animationLoop=this._createAnimationLoop(n,e),this.setProps(t),e._typedArrayManagerProps&&Jf.setOptions(e._typedArrayManagerProps),this.animationLoop.start()}finalize(){this._restoreDeviceResizeHandler(),this.animationLoop?.stop(),this.animationLoop?.destroy(),this.animationLoop=null,this._hoverPickSequence++,this._pointerDownPickSequence++,this._lastPointerDownInfo=null,this._lastPointerDownInfoPromise=null,this.layerManager?.finalize(),this.layerManager=null,this.viewManager?.finalize(),this.viewManager=null,this.effectManager?.finalize(),this.effectManager=null,this.deckRenderer?.finalize(),this.deckRenderer=null,this.deckPicker?.finalize(),this.deckPicker=null,Object.keys(this._canvasManager.targets).length||this.eventManager?.destroy(),this.eventManager=null,this.eventManagers={},this.widgetManager?.finalize(),this.widgetManager=null,this._canvasManager.finalize(),this._isMultiCanvasMode()?this.canvas=null:this.canvas&&this.canvas===this._ownedCanvas&&(this.canvas.parentElement?.removeChild(this.canvas),this.canvas=null,this._ownedCanvas=null),this._canvasContext=null}setProps(e){this.stats.get(`setProps Time`).timeStart(),`onLayerHover`in e&&z.removed(`onLayerHover`,`onHover`)(),`onLayerClick`in e&&z.removed(`onLayerClick`,`onClick`)(),e.initialViewState&&!Ip(this.props.initialViewState,e.initialViewState,3)&&(this.viewState=e.initialViewState),H(!(`_canvases`in e)||Array.isArray(e._canvases)===this._isMultiCanvasMode()),Object.assign(this.props,e),this._validateCanvasConfiguration(this.props),this._validateInternalPickingMode(),this.device&&this._isMultiCanvasMode()&&this._syncCanvasTargets(),this._setCanvasSize(this.props);let t=Object.create(this.props);if(Object.assign(t,{views:this._getViews(),width:this.width,height:this.height,viewState:this._getViewState(),eventManagers:this.eventManagers}),e.device&&e.device.id!==this.device?.id){let t=e.device.getDefaultCanvasContext();this.animationLoop?.stop(),!this._isMultiCanvasMode()&&this.canvas!==t.canvas&&(this.canvas?.remove(),this.eventManager?.destroy(),this.canvas=null),this._setDeviceCanvasContext(e.device),z.log(`recreating animation loop for new device! id=${e.device.id}`)(),this.animationLoop=this._createAnimationLoop(e.device,e),this.animationLoop.start()}if(this.animationLoop?.setProps(t),e.useDevicePixels!==void 0&&this._canvasContext?.setProps){this._canvasContext.setProps({useDevicePixels:e.useDevicePixels});for(let t of Object.values(this._canvasManager.targets))t.presentationContext.setProps({useDevicePixels:e.useDevicePixels})}this.layerManager&&(this.viewManager.setProps(t),this.layerManager.activateViewport(this.getViewports()[0]),this.layerManager.setProps(t),this.effectManager.setProps(t),this.deckRenderer.setProps(t),this.deckPicker.setProps(t),this.widgetManager.setProps(t)),this.stats.get(`setProps Time`).timeEnd()}needsRedraw(e={clearRedrawFlags:!1}){if(!this.layerManager)return!1;if(this.props._animate)return`Deck._animate`;let t=this._needsRedraw;e.clearRedrawFlags&&(this._needsRedraw=!1);let n=this.viewManager.needsRedraw(e),r=this.layerManager.needsRedraw(e),i=this.effectManager.needsRedraw(e),a=this.deckRenderer.needsRedraw(e);return t=t||n||r||i||a,t}redraw(e){if(!this.layerManager)return;let t=this.needsRedraw({clearRedrawFlags:!0});t=e||t,t&&(this.stats.get(`Redraw Count`).incrementCount(),this.props._customRender?this.props._customRender(t):this._drawLayers(t))}get isInitialized(){return this.viewManager!==null}getViews(){return H(this.viewManager),this.viewManager.views}getView(e){return H(this.viewManager),this.viewManager.getView(e)}getViewports(e){return H(this.viewManager),this.viewManager.getViewports(e)}getCanvas(){return this.canvas}getCanvasContext(e){let t=e?this.viewManager?.getView(e)?.props.canvasId:void 0;return this._getCanvasContext(t)}getEventManager(e){if(!e||!this.viewManager)return this.eventManager;let t=this.viewManager.getCanvasId(e)||`default-canvas`;return this.eventManagers[t]||this.eventManager}async pickObjectAsync(e){let t=(await this._pickAsync(`pickObjectAsync`,`pickObject Time`,e)).result;return t.length?t[0]:null}async pickObjectsAsync(e){return await this._pickAsync(`pickObjectsAsync`,`pickObjects Time`,e)}pickObject(e){let t=this._pick(`pickObject`,`pickObject Time`,e).result;return t.length?t[0]:null}pickMultipleObjects(e){return e.depth=e.depth||10,this._pick(`pickObject`,`pickMultipleObjects Time`,e).result}pickObjects(e){return this._pick(`pickObjects`,`pickObjects Time`,e)}_pickPositionForController(e,t,n){return this._getInternalPickingMode()===`sync`?this.pickObject({x:e,y:t,radius:0,unproject3D:!0,canvasId:n?this.viewManager?.getCanvasId(n):void 0}):null}_addResources(e,t=!1){for(let n in e)this.layerManager.resourceManager.add({resourceId:n,data:e[n],forceUpdate:t})}_removeResources(e){for(let t of e)this.layerManager.resourceManager.remove(t)}_addDefaultEffect(e){this.effectManager.addDefaultEffect(e)}_addDefaultShaderModule(e){this.layerManager.addDefaultShaderModule(e)}_removeDefaultShaderModule(e){this.layerManager?.removeDefaultShaderModule(e)}_resolveInternalPickingMode(){let{pickAsync:e}=this.props,t=this.device?.type||this.props.deviceProps?.type;if(e===`auto`)return t===`webgpu`?`async`:`sync`;if(e===`sync`&&t===`webgpu`)throw Error('`pickAsync: "sync"` is not supported when Deck is using a WebGPU device.');return e}_getInternalPickingMode(){try{return this._resolveInternalPickingMode()}catch(e){return this.props.onError?.(e),null}}_validateInternalPickingMode(){this._getInternalPickingMode()}_getFirstPickedInfo({result:e,emptyInfo:t}){return e[0]||t}_shouldUnproject3D(e=this.layerManager?.getLayers()||[]){return e.some(e=>e.props.pickable===`3d`)}_getPointPickOptions(e,t,n={},r=this.layerManager?.getLayers()||[]){return{x:e,y:t,canvasId:n.canvasId,radius:this.props.pickingRadius,unproject3D:this._shouldUnproject3D(r),...n}}_pickPointSync(e){return this._pick(`pickObject`,`pickObject Time`,e)}_pickPointAsync(e){return this._pickAsync(`pickObjectAsync`,`pickObject Time`,e)}_getLastPointerDownPickingInfo(e,t,n,r=this.layerManager?.getLayers()||[]){return this.deckPicker.getLastPickedObject({x:e,y:t,layers:r,viewports:this.getViewports({x:e,y:t,canvasId:n})},this._lastPointerDownInfo)}_applyHoverCallbacks({result:e,emptyInfo:t},n){if(!this.widgetManager)return;this.cursorState.isHovering=e.length>0;let r=t,i=!1;for(let t of e)r=t,i=t.layer?.onHover(t,n)||i;i||(this.props.onHover?.(r,n),this.widgetManager.onHover(r,n))}_dispatchPickingEvent(e,t){if(!this.layerManager||!this.widgetManager)return;let n=rd[t.type];if(!n)return;let{layer:r}=e,i=r&&(r[n]||r.props[n]),a=this.props[n],o=!1;i&&(o=i.call(r,e,t)),o||(a?.(e,t),this.widgetManager.onEvent(e,t))}_pickAsync(e,t,n){H(this.deckPicker);let{stats:r}=this,i=this._isMultiCanvasMode()?n.canvasId||this._getDefaultCanvasId():n.canvasId,a=this._getCanvasContext(i)||void 0;r.get(`Pick Count`).incrementCount(),r.get(t).timeStart(),this._resizeForCanvasTarget(i);let o=this.deckPicker[e]({layers:this.layerManager.getLayers(n),views:this.viewManager.getViews(),viewports:this.getViewports({...n,canvasId:i}),onViewportActive:this.layerManager.activateViewport,effects:this.effectManager.getEffects(),...n,canvasId:i,canvasContext:a});return r.get(t).timeEnd(),o}_pick(e,t,n){H(this.deckPicker);let{stats:r}=this,i=this._isMultiCanvasMode()?n.canvasId||this._getDefaultCanvasId():n.canvasId,a=this._getCanvasContext(i)||void 0;r.get(`Pick Count`).incrementCount(),r.get(t).timeStart(),this._resizeForCanvasTarget(i);let o=this.deckPicker[e]({layers:this.layerManager.getLayers(n),views:this.viewManager.getViews(),viewports:this.getViewports({...n,canvasId:i}),onViewportActive:this.layerManager.activateViewport,effects:this.effectManager.getEffects(),...n,canvasId:i,canvasContext:a});return r.get(t).timeEnd(),o}_createCanvas(e){let t=e.canvas;return typeof t==`string`&&(t=document.getElementById(t),H(t)),t?this._ownedCanvas=null:(t=document.createElement(`canvas`),t.id=e.id||`deckgl-overlay`,e.width&&typeof e.width==`number`&&(t.width=e.width),e.height&&typeof e.height==`number`&&(t.height=e.height),(e.parent||document.body).appendChild(t),this._ownedCanvas=t),Object.assign(t.style,e.style),t}_isMultiCanvasMode(){return Array.isArray(this.props._canvases)}_getDefaultCanvasId(){return this._canvasManager.order[0]||`default-canvas`}_validateCanvasConfiguration(e){Array.isArray(e._canvases)&&(H(!e.canvas),H(!e.gl),H(!e.device?.canvasContext||e.device.getDefaultCanvasContext().offscreenCanvas))}_createEventManager(e){let t=new $u(e,{touchAction:this.props.touchAction,recognizers:Object.keys(id).map(e=>{let[t,n,r,i]=id[e],a=this.props.eventRecognizerOptions?.[e];return{recognizer:new t({...n,...a,event:e}),recognizeWith:r,requireFailure:i}}),events:{pointerdown:this._onPointerDown,pointermove:this._onPointerMove,pointerleave:this._onPointerMove}});for(let e in rd)e===`dblclick`?t.watch(e,this._onEvent):t.on(e,this._onEvent);return t}_getEventRoot(e){return e.closest(`.deck-events-root`)||this.props.parent?.querySelector(`.deck-events-root`)||e}_syncCanvasTargets(){if(!this.device||!this._isMultiCanvasMode())return;this._canvasManager.syncCanvasEntries({device:this.device,canvases:this.props._canvases||[],useDevicePixels:this.props.useDevicePixels}),this.eventManagers=this._canvasManager.eventManagers;let e=this._getDefaultCanvasId();this.eventManager=this.eventManagers[e]||null,this.canvas=this._canvasManager.targets[e]?.canvas||null}_setCanvasContext(e){this._canvasContext=e,`style`in e.canvas&&(this.canvas=e.canvas)}_setDeviceCanvasContext(e,t={}){let n=e.getDefaultCanvasContext();this._setCanvasContext(n),this._setDeviceResizeHandler(e,t)}_setDeviceResizeHandler(e,t={}){let n=!!t.syncDrawingBuffer;if(this._deviceResizeHandler?.device===e){this._deviceResizeHandler.syncDrawingBuffer=n;return}this._restoreDeviceResizeHandler();let r=e=>{this._isMultiCanvasMode()?this._updateMultiCanvasDimensions():e===this._canvasContext&&this._canvasContext&&this._onCanvasContextResize(this._canvasContext,{syncDrawingBuffer:this._deviceResizeHandler?.syncDrawingBuffer})};e.props.onResize=r,this._deviceResizeHandler={device:e,onResize:r,syncDrawingBuffer:n}}_restoreDeviceResizeHandler(){let e=this._deviceResizeHandler;e&&e.device.props?.onResize===e.onResize&&(e.device.props.onResize=th),this._deviceResizeHandler=null}_setCanvasSize(e){if(this._isMultiCanvasMode()||!this.canvas)return;let{width:t,height:n}=e;if(t||t===0){let e=Number.isFinite(t)?`${t}px`:t;this.canvas.style.width=e}if(n||n===0){let t=Number.isFinite(n)?`${n}px`:n;this.canvas.style.position=e.style?.position||`absolute`,this.canvas.style.height=t}}_getCanvasIdFromEvent(e){return this._canvasManager.getCanvasIdFromEvent(e?.rootElement)}_getCanvasContext(e){return this._canvasManager.getTarget(e)?.presentationContext||this._canvasContext}_resizeForCanvasTarget(e){let t=this._canvasManager.getTarget(e);if(!t||!this.device?.canvasContext)return;let[n,r]=t.presentationContext.getDrawingBufferSize();this.device.canvasContext.setDrawingBufferSize(n,r)}_createDeviceCanvas(e){if(this._isMultiCanvasMode()){let t=globalThis.OffscreenCanvas;if(!t)throw Error("`_canvases` requires OffscreenCanvas support.");return new t(typeof e.width==`number`&&Number.isFinite(e.width)?e.width:1,typeof e.height==`number`&&Number.isFinite(e.height)?e.height:1)}return this._createCanvas(e)}_updateCanvasSize(e=this._canvasContext){if(this._isMultiCanvasMode()){this._updateMultiCanvasDimensions();return}let{canvas:t}=this,[n,r]=e?e.getCSSSize():[t?.clientWidth??t?.width??0,t?.clientHeight??t?.height??0];(n!==this.width||r!==this.height)&&(this.width=n,this.height=r,this.viewManager?.setProps({width:n,height:r}),this.layerManager?.activateViewport(this.getViewports()[0]),this.props.onResize({width:n,height:r},e||void 0))}_onCanvasContextResize(e,t={}){if(t.syncDrawingBuffer){let{width:t,height:n}=e.canvas;e.setDrawingBufferSize(t,n)}this._needsRedraw=`Canvas resized`,this._updateCanvasSize(e)}_updateMultiCanvasDimensions(){let[e,t]=this._getCanvasContext()?.getCSSSize()||[0,0];(e!==this.width||t!==this.height)&&(this.width=e,this.height=t,this.props.onResize({width:e,height:t})),this._needsRedraw=`Canvas resized`,this.viewManager?.setNeedsUpdate(`Canvas resized`),this.viewManager?.setProps({width:this.width,height:this.height})}_createAnimationLoop(e,t){let{gl:n,onError:r}=t;return new Ei({device:e,autoResizeDrawingBuffer:!n&&!Array.isArray(t._canvases),autoResizeViewport:!1,onInitialize:e=>this._setDevice(e.device),onRender:this._onRenderFrame.bind(this),onError:r})}_createDevice(e){let t=this.props.deviceProps?.createCanvasContext,n=typeof t==`object`?t:void 0,r={adapters:[],_cacheShaders:!0,_cachePipelines:!0,...e.deviceProps};r.adapters.includes(ya)||r.adapters.push(ya);let i={alphaMode:this.props.deviceProps?.type===`webgpu`?`premultiplied`:void 0};return ye.createDevice({_reuseDevices:!0,type:`webgl`,...r,createCanvasContext:{...i,...n,canvas:this._createDeviceCanvas(e),useDevicePixels:this.props.useDevicePixels,autoResize:!0}})}_getViewState(){return this.props.viewState||this.viewState}_getViews(){let{views:e}=this.props,t=Array.isArray(e)?e:e?[e]:[new Am({id:`default-view`})];return t.length&&this.props.controller&&(t[0]=t[0].clone({controller:this.props.controller})),t}_onContextLost(){let{onError:e}=this.props;this.animationLoop&&e&&e(Error(`WebGL context is lost`))}_pickAndCallback(){let{_pickRequest:e}=this;if(e.event){let t=e.event,n=this.layerManager?.getLayers()||[],r=this._getPointPickOptions(e.x,e.y,{canvasId:e.canvasId,radius:e.radius,mode:e.mode},n),i=this._getInternalPickingMode(),a=++this._hoverPickSequence;if(e.event=null,e.canvasId=void 0,!i)return;if(i===`sync`){this._applyHoverCallbacks(this._pickPointSync(r),t);return}this._pickPointAsync(r).then(({result:e,emptyInfo:n})=>{a===this._hoverPickSequence&&this._applyHoverCallbacks({result:e,emptyInfo:n},t)}).catch(e=>this.props.onError?.(e))}}_updateCursor(){let e=this.props.getCursor(this.cursorState);if(this._isMultiCanvasMode()){for(let t of Object.values(this._canvasManager.targets))t.canvas.style.cursor=e;return}let t=this.props.parent||this.canvas;t&&(t.style.cursor=e)}_setDevice(e){if(this.device=e,this._validateInternalPickingMode(),!this.animationLoop)return;this._setDeviceCanvasContext(e,{syncDrawingBuffer:!!(this.props.gl&&this.props.device!==e)}),this._isMultiCanvasMode()?this._syncCanvasTargets():this.canvas&&!this.canvas.isConnected&&this.props.parent&&this.props.parent.insertBefore(this.canvas,this.props.parent.firstChild),this.device.type===`webgl`&&this.device.setParametersWebGL({blend:!0,blendFunc:[770,771,1,771],polygonOffsetFill:!0,depthTest:!0,depthFunc:515}),this.props.onDeviceInitialized(this.device),this.device.type===`webgl`&&this.props.onWebGLInitialized(this.device.gl);let t=new bi;if(t.play(),this.animationLoop.attachTimeline(t),!this._isMultiCanvasMode()){let e=this.canvas&&this._getEventRoot(this.canvas);H(e),this.eventManager=this._createEventManager(e),this.eventManagers={[Lp]:this.eventManager}}this.viewManager=new Rp({timeline:t,eventManager:this.eventManager,eventManagers:this.eventManagers,getCanvasContext:this._isMultiCanvasMode()?this.getCanvasContext.bind(this):void 0,onViewStateChange:this._onViewStateChange.bind(this),onInteractionStateChange:this._onInteractionStateChange.bind(this),pickPosition:this._pickPositionForController.bind(this),views:this._getViews(),viewState:this._getViewState(),width:this.width,height:this.height});let n=this.viewManager.getViewports()[0];this.layerManager=new Fp(this.device,{deck:this,stats:this.stats,viewport:n,timeline:t}),this.effectManager=new Nm({deck:this,device:this.device}),this.deckRenderer=new Im(this.device,{stats:this.stats}),this.deckPicker=new Wm(this.device,{stats:this.stats});let r=this.props.parent?.querySelector(`.deck-widgets-root`)||(this._isMultiCanvasMode()?this.props.parent||this.canvas?.parentElement:null)||this.canvas?.parentElement;this.widgetManager=new Jm({deck:this,parentElement:r}),this.widgetManager.addDefault(new $m),this.setProps({}),this._updateCanvasSize(this._canvasContext),this.props.onLoad()}_drawLayers(e,t){let{device:n,gl:r}=this.layerManager.context;this.props.onBeforeRender({device:n,gl:r});let i={target:this.props._framebuffer,layers:this.layerManager.getLayers(),viewports:this.viewManager.getViewports(),onViewportActive:this.layerManager.activateViewport,views:this.viewManager.getViews(),pass:`screen`,effects:this.effectManager.getEffects(),...t};if(this._isMultiCanvasMode()&&i.pass===`screen`&&!i.target&&this._canvasManager.order.length)for(let e of this._canvasManager.order){let t=i.viewports.filter(t=>this.viewManager.getCanvasId(t.id)===e);if(!t.length){let t=this._canvasManager.targets[e];this._resizeForCanvasTarget(e),this.deckRenderer?.renderLayers({...i,canvasContext:t.presentationContext,target:t.presentationContext.getCurrentFramebuffer(),viewports:[],clearCanvas:!0}),t.presentationContext.present();continue}let n=this._canvasManager.targets[e];this._resizeForCanvasTarget(e);let r=n.presentationContext.getCurrentFramebuffer();this.deckRenderer?.renderLayers({...i,canvasContext:n.presentationContext,target:r,viewports:t}),n.presentationContext.present()}else this.deckRenderer?.renderLayers(i);i.pass===`screen`&&this.widgetManager.onRedraw({viewports:i.viewports,layers:i.layers}),this.props.onAfterRender({device:n,gl:r})}_onRenderFrame(){this._getFrameStats(),this._metricsCounter++%60==0&&(this._getMetrics(),this.stats.reset(),z.table(4,this.metrics)(),this.props._onMetrics&&this.props._onMetrics(this.metrics)),this._updateCursor(),this.layerManager.updateLayers(),this._pickAndCallback(),this.redraw(),this.viewManager&&this.viewManager.updateViewStates()}_onViewStateChange(e){let t=this.props.onViewStateChange(e)||e.viewState;this.viewState&&(this.viewState={...this.viewState,[e.viewId]:t},this.props.viewState||this.viewManager&&this.viewManager.setProps({viewState:this.viewState}))}_onInteractionStateChange(e){this.cursorState.isDragging=e.isDragging||!1,this.props.onInteractionStateChange(e)}_getFrameStats(){let{stats:e}=this;e.get(`frameRate`).timeEnd(),e.get(`frameRate`).timeStart();let t=this.animationLoop.stats;e.get(`GPU Time`).addTime(t.get(`GPU Time`).lastTiming),e.get(`CPU Time`).addTime(t.get(`CPU Time`).lastTiming)}_getMetrics(){let{metrics:e,stats:t}=this;e.fps=t.get(`frameRate`).getHz(),e.setPropsTime=t.get(`setProps Time`).time,e.updateAttributesTime=t.get(`Update Attributes`).time,e.framesRedrawn=t.get(`Redraw Count`).count,e.pickTime=t.get(`pickObject Time`).time+t.get(`pickMultipleObjects Time`).time+t.get(`pickObjects Time`).time,e.pickCount=t.get(`Pick Count`).count,e.layersCount=this.layerManager?.layers.length??0,e.drawLayersCount=t.get(`Layers rendered`).lastSampleCount,e.pickLayersCount=t.get(`Layers picked`).lastSampleCount,e.updateLayersCount=t.get(`Layer updates`).count,e.updateAttributesCount=t.get(`Attributes updated`).count,e.gpuTime=t.get(`GPU Time`).time,e.cpuTime=t.get(`CPU Time`).time,e.gpuTimePerFrame=t.get(`GPU Time`).getAverageTime(),e.cpuTimePerFrame=t.get(`CPU Time`).getAverageTime();let n=ye.stats.get(`GPU Time and Memory`);e.bufferMemory=n.get(`Buffer Memory`).count,e.textureMemory=n.get(`Texture Memory`).count,e.renderbufferMemory=n.get(`Renderbuffer Memory`).count,e.gpuMemory=n.get(`GPU Memory`).count}};rh.defaultProps=nh,rh.VERSION=il;function ih(e){switch(e){case`float64`:return Float64Array;case`uint8`:case`unorm8`:return Uint8ClampedArray;default:return T(e)}}var ah=E.getDataType.bind(E);function oh(e,t,n){if(t.size>4)return null;let r=n===`webgpu`&&t.type===`uint8`?`unorm8`:t.type,i=t.size,a=!!(n!==`webgpu`&&i===3&&r&&[`uint8`,`sint8`,`unorm8`,`snorm8`,`uint16`,`sint16`,`unorm16`,`snorm16`].includes(r));return{attribute:e,format:i>1?`${r}x${i}${a?`-webgl`:``}`:t.type,byteOffset:t.offset||0}}function sh(e){return e.stride||e.size*e.bytesPerElement}function ch(e,t){return e.type===t.type&&e.size===t.size&&sh(e)===sh(t)&&(e.offset||0)===(t.offset||0)}function lh(e,t){t.offset&&z.removed(`shaderAttribute.offset`,`vertexOffset, elementOffset`)();let n=sh(e),r=t.vertexOffset===void 0?e.vertexOffset||0:t.vertexOffset,i=t.elementOffset||0,a=r*n+i*e.bytesPerElement+(e.offset||0);return{...t,offset:a,stride:n}}function uh(e,t){let n=lh(e,t);return{high:n,low:{...n,offset:n.offset+e.size*4}}}var dh=class{constructor(e,t,n){this._buffer=null,this.device=e,this.id=t.id||``,this.size=t.size||1;let r=t.logicalType||t.type,i=r===`float64`,{defaultValue:a}=t;a=Number.isFinite(a)?[a]:a||Array(this.size).fill(0);let o;o=i?`float32`:!r&&t.isIndexed?`uint32`:r||`float32`;let s=ih(r||o);this.doublePrecision=i,i&&t.fp64===!1&&(s=Float32Array),this.value=null,this.settings={...t,defaultType:s,defaultValue:a,logicalType:r,type:o,normalized:o.includes(`norm`),size:this.size,bytesPerElement:s.BYTES_PER_ELEMENT},this.state={...n,externalBuffer:null,bufferAccessor:this.settings,allocatedValue:null,numInstances:0,bounds:null,constant:!1}}get isConstant(){return this.state.constant}get buffer(){return this._buffer}get byteOffset(){let e=this.getAccessor();return e.vertexOffset?e.vertexOffset*sh(e):0}get numInstances(){return this.state.numInstances}set numInstances(e){this.state.numInstances=e}get isDoublePrecisionBuffer(){return this._shouldSplitDoublePrecisionValue(this.value)}delete(){this._buffer&&=(this._buffer.delete(),null),Jf.release(this.state.allocatedValue),this.state.allocatedValue=null}getBuffer(){return this.state.constant&&this.device.type!==`webgpu`?null:this.state.externalBuffer||this._buffer}getValue(e=this.id,t=null){let n={};if(this.state.constant){let r=this.value;if(this.device.type===`webgpu`&&this._buffer)n[e]=this._buffer;else if(t){let i=lh(this.getAccessor(),t),a=i.offset/r.BYTES_PER_ELEMENT,o=i.size||this.size;n[e]=r.subarray(a,a+o)}else n[e]=r}else n[e]=this.getBuffer();return this.doublePrecision&&(this.isDoublePrecisionBuffer?n[`${e}64Low`]=n[e]:n[`${e}64Low`]=new Float32Array(this.size)),n}_getBufferLayout(e=this.id,t=null){let n=this.getAccessor(),r=[],i={name:this.id,byteStride:this.device.type===`webgpu`&&this.state.constant?0:sh(n)};if(this.doublePrecision){let i=uh(n,t||{});r.push(oh(e,{...n,...i.high},this.device.type),oh(`${e}64Low`,{...n,...i.low},this.device.type))}else if(t){let i=lh(n,t);r.push(oh(e,{...n,...i},this.device.type))}else r.push(oh(e,n,this.device.type));return i.attributes=r.filter(Boolean),i}setAccessor(e){this.state.bufferAccessor=e}getAccessor(){return this.state.bufferAccessor}getBounds(){if(this.state.bounds)return this.state.bounds;let e=null;if(this.state.constant&&this.value){let t=Array.from(this.value);e=[t,t]}else{let{value:t,numInstances:n,size:r}=this,i=n*r;if(t&&i&&t.length>=i){let n=Array(r).fill(1/0),a=Array(r).fill(-1/0);for(let e=0;e<i;)for(let i=0;i<r;i++){let r=t[e++];r<n[i]&&(n[i]=r),r>a[i]&&(a[i]=r)}e=[n,a]}}return this.state.bounds=e,e}setData(e){let{state:t}=this,n;n=ArrayBuffer.isView(e)?{value:e}:e instanceof C?{buffer:e}:e;let r={...this.settings,...n};if(ArrayBuffer.isView(n.value)){if(!n.type)if(this.doublePrecision&&n.value instanceof Float64Array)r.type=`float32`;else{let e=ah(n.value);r.type=r.normalized?e.replace(`int`,`norm`):e}r.bytesPerElement=n.value.BYTES_PER_ELEMENT,r.stride=sh(r)}if(t.bounds=null,n.constant){let e=n.value;if(e=this._normalizeValue(e,[],0),this.settings.normalized&&(e=this.normalizeConstant(e)),!(!t.constant||!this._areValuesEqual(e,this.value)))return!1;t.externalBuffer=null,t.constant=!0,this.value=ArrayBuffer.isView(e)?e:new Float32Array(e)}else if(n.buffer)t.externalBuffer=n.buffer,t.constant=!1,this.value=n.value||null;else if(n.value){this._checkExternalBuffer(n);let e=n.value,i=e;t.externalBuffer=null,t.constant=!1,this.value=e,this._shouldSplitDoublePrecisionValue(i)&&(i=rp(i,r),e instanceof Float32Array&&(r.stride=r.size*2*Float32Array.BYTES_PER_ELEMENT));let{buffer:a}=this,o=sh(r),s=(r.vertexOffset||0)*o;if(this.settings.isIndexed){let e=this.settings.defaultType;i.constructor!==e&&(i=new e(i))}let c=i.byteLength+s+o*2;(!a||a.byteLength<c)&&(a=this._createBuffer(c)),a.write(i,s)}return this.setAccessor(r),!0}updateSubBuffer(e={}){this.state.bounds=null;let t=this.value,{startOffset:n=0,endOffset:r}=e,i=this._shouldSplitDoublePrecisionValue(t);this.buffer.write(i?rp(t,{size:this.size,startIndex:n,endIndex:r}):t.subarray(n,r),n*(i?8:t.BYTES_PER_ELEMENT)+this.byteOffset)}allocate(e,t=!1){let{state:n}=this,r=n.allocatedValue,i=Jf.allocate(r,e+1,{size:this.size,type:this.settings.defaultType,copy:t});this.value=i;let a=this._shouldSplitDoublePrecisionValue(i),o=a&&i instanceof Float32Array?{...this.settings,stride:this.size*2*Float32Array.BYTES_PER_ELEMENT}:this.settings;this.setAccessor(o);let{byteOffset:s}=this,{buffer:c}=this,l=i.byteLength*(a&&i instanceof Float32Array?2:1);return(!c||c.byteLength<l+s)&&(c=this._createBuffer(l+s),t&&r&&c.write(this._shouldSplitDoublePrecisionValue(r)?rp(r,this):r,s)),n.allocatedValue=i,n.constant=!1,n.externalBuffer=null,!0}_shouldSplitDoublePrecisionValue(e){return!!(this.doublePrecision&&(e instanceof Float64Array||this.device.type===`webgpu`&&e instanceof Float32Array))}_checkExternalBuffer(e){let{value:t}=e;if(!ArrayBuffer.isView(t))throw Error(`Attribute ${this.id} value is not TypedArray`);let n=this.settings.defaultType,r=!1;if(this.doublePrecision&&(r=t.BYTES_PER_ELEMENT<4),r)throw Error(`Attribute ${this.id} does not support ${t.constructor.name}`);!(t instanceof n)&&this.settings.normalized&&!(`normalized`in e)&&z.warn(`Attribute ${this.id} is normalized`)()}normalizeConstant(e){switch(this.settings.type){case`snorm8`:return new Float32Array(e).map(e=>(e+128)/255*2-1);case`snorm16`:return new Float32Array(e).map(e=>(e+32768)/65535*2-1);case`unorm8`:return new Float32Array(e).map(e=>e/255);case`unorm16`:return new Float32Array(e).map(e=>e/65535);default:return e}}_normalizeValue(e,t,n){let{defaultValue:r,size:i}=this.settings;if(Number.isFinite(e))return t[n]=e,t;if(!e){let e=i;for(;--e>=0;)t[n+e]=r[e];return t}switch(i){case 4:t[n+3]=Number.isFinite(e[3])?e[3]:r[3];case 3:t[n+2]=Number.isFinite(e[2])?e[2]:r[2];case 2:t[n+1]=Number.isFinite(e[1])?e[1]:r[1];case 1:t[n+0]=Number.isFinite(e[0])?e[0]:r[0];break;default:let a=i;for(;--a>=0;)t[n+a]=Number.isFinite(e[a])?e[a]:r[a]}return t}_areValuesEqual(e,t){if(!e||!t)return!1;let{size:n}=this;for(let r=0;r<n;r++)if(e[r]!==t[r])return!1;return!0}_createBuffer(e){this._buffer&&this._buffer.destroy();let{isIndexed:t,type:n}=this.settings,r=this.device.type===`webgpu`&&!t?C.VERTEX|C.STORAGE|C.COPY_DST|C.COPY_SRC:(t?C.INDEX:C.VERTEX)|C.COPY_DST;return this._buffer=this.device.createBuffer({...this._buffer?.props,id:this.id,usage:r,indexType:t?n:void 0,byteLength:e}),this._buffer}},fh=[],ph=[];function mh(e,t=0,n=1/0){let r=fh,i={index:-1,data:e,target:[]};return e?typeof e[Symbol.iterator]==`function`?r=e:e.length>0&&(ph.length=e.length,r=ph):r=fh,(t>0||Number.isFinite(n))&&(r=(Array.isArray(r)?r:Array.from(r)).slice(t,n),i.index=t-1),{iterable:r,objectInfo:i}}function hh(e){return e&&e[Symbol.asyncIterator]}function gh(e,t){let{size:n,stride:r,offset:i,startIndices:a,nested:o}=t,s=e.BYTES_PER_ELEMENT,c=r?r/s:n,l=i?i/s:0,u=Math.floor((e.length-l)/c);return(t,{index:r,target:i})=>{if(!a){let t=r*c+l;for(let r=0;r<n;r++)i[r]=e[t+r];return i}let s=a[r],d=a[r+1]||u,f;if(o){f=Array(d-s);for(let t=s;t<d;t++){let r=t*c+l;i=Array(n);for(let t=0;t<n;t++)i[t]=e[r+t];f[t-s]=i}}else if(c===n)f=e.subarray(s*n+l,d*n+l);else{f=new e.constructor((d-s)*n);let t=0;for(let r=s;r<d;r++){let i=r*c+l;for(let r=0;r<n;r++)f[t++]=e[i+r]}}return f}}var _h=[],vh=[[0,1/0]];function yh(e,t){if(e===vh||(t[0]<0&&(t[0]=0),t[0]>=t[1]))return e;let n=[],r=e.length,i=0;for(let a=0;a<r;a++){let r=e[a];r[1]<t[0]?(n.push(r),i=a+1):r[0]>t[1]?n.push(r):t=[Math.min(r[0],t[0]),Math.max(r[1],t[1])]}return n.splice(i,0,t),n}var bh={interpolation:{duration:0,easing:e=>e},spring:{stiffness:.05,damping:.5}};function xh(e,t){if(!e)return null;Number.isFinite(e)&&(e={type:`interpolation`,duration:e});let n=e.type||`interpolation`;return{...bh[n],...t,...e,type:n}}var Sh=class extends dh{constructor(e,t){super(e,t,{startIndices:null,constantValue:null,lastExternalBuffer:null,binaryValue:null,binaryAccessor:null,needsUpdate:!0,needsRedraw:!1,layoutChanged:!1,updateRanges:vh}),this.constant=!1,this.settings.update=t.update||(t.accessor?this._autoUpdater:void 0),Object.seal(this.settings),Object.seal(this.state),this._validateAttributeUpdaters()}get startIndices(){return this.state.startIndices}set startIndices(e){this.state.startIndices=e}needsUpdate(){return this.state.needsUpdate}needsRedraw({clearChangedFlags:e=!1}={}){let t=this.state.needsRedraw;return this.state.needsRedraw=t&&!e,t}layoutChanged(){return this.state.layoutChanged}setAccessor(e){var t;(t=this.state).layoutChanged||(t.layoutChanged=!ch(e,this.getAccessor())),super.setAccessor(e)}getUpdateTriggers(){let{accessor:e}=this.settings;return[this.id].concat(typeof e!=`function`&&e||[])}supportsTransition(){return!!this.settings.transition}getTransitionSetting(e){if(!e||!this.supportsTransition())return null;let{accessor:t}=this.settings,n=this.settings.transition;return xh(Array.isArray(t)?e[t.find(t=>e[t])]:e[t],n)}setNeedsUpdate(e=this.id,t){if(this.state.needsUpdate=this.state.needsUpdate||e,this.setNeedsRedraw(e),t){let{startRow:e=0,endRow:n=1/0}=t;this.state.updateRanges=yh(this.state.updateRanges,[e,n])}else this.state.updateRanges=vh}clearNeedsUpdate(){this.state.needsUpdate=!1,this.state.updateRanges=_h}setNeedsRedraw(e=this.id){this.state.needsRedraw=this.state.needsRedraw||e}allocate(e){let{state:t,settings:n}=this;if(n.noAlloc)return!1;if(n.update){let n=this.isConstant;return super.allocate(e,t.updateRanges!==vh),t.layoutChanged||=n&&this.device.type===`webgpu`,!0}return!1}updateBuffer({numInstances:e,data:t,props:n,context:r}){if(!this.needsUpdate())return!1;let{state:{updateRanges:i},settings:{update:a,noAlloc:o}}=this,s=!0;if(a){for(let[o,s]of i)a.call(r,this,{data:t,startRow:o,endRow:s,props:n,numInstances:e});if(this.value)if(this.constant||!this.buffer||this.buffer.byteLength<this.value.byteLength+this.byteOffset){if(this.constant){let e=this.value;this.value=null,this.setConstantValue(r,e)}else this.setData({value:this.value,constant:this.constant});this.constant=!1}else for(let[t,n]of i){let r=Number.isFinite(t)?this.getVertexOffset(t):0,i=Number.isFinite(n)?this.getVertexOffset(n):o||!Number.isFinite(e)?this.value.length:e*this.size;super.updateSubBuffer({startOffset:r,endOffset:i})}this._checkAttributeArray()}else s=!1;return this.clearNeedsUpdate(),this.setNeedsRedraw(),s}setConstantValue(e,t){var n;if(t===void 0||typeof t==`function`)return!1;let r=this.isConstant,i=this.settings.transform&&e?this.settings.transform.call(e,t):t,a=this.settings.defaultType;this.state.constantValue=this._normalizeValue(i,new a(this.size),0);let o=this.setData({constant:!0,value:i});if(this.device.type===`webgpu`){let e=this.state.constantValue;this.doublePrecision&&(e instanceof Float32Array||e instanceof Float64Array)&&(e=rp(e,{size:this.size}),this.setAccessor({...this.getAccessor(),stride:this.size*2*Float32Array.BYTES_PER_ELEMENT}));let t=this._buffer;(!t||t.byteLength<e.byteLength)&&(t=this._createBuffer(e.byteLength)),t.write(e),(n=this.state).layoutChanged||(n.layoutChanged=!r),this.constant=!1}return o&&this.setNeedsRedraw(),this.clearNeedsUpdate(),!0}getConstantValue(){return this.isConstant?this.state.constantValue:null}setExternalBuffer(e){let{state:t}=this;return e?(this.clearNeedsUpdate(),t.lastExternalBuffer===e?!0:(t.lastExternalBuffer=e,this.setNeedsRedraw(),this.setData(e),!0)):(t.lastExternalBuffer=null,!1)}setBinaryValue(e,t=null){let{state:n,settings:r}=this;if(!e)return n.binaryValue=null,n.binaryAccessor=null,!1;if(r.noAlloc)return!1;if(n.binaryValue===e)return this.clearNeedsUpdate(),!0;if(n.binaryValue=e,this.setNeedsRedraw(),r.transform||t!==this.startIndices){ArrayBuffer.isView(e)&&(e={value:e});let i=e;H(ArrayBuffer.isView(i.value),`invalid ${r.accessor}`);let a=!!i.size&&i.size!==this.size;return n.binaryAccessor=gh(i.value,{size:i.size||this.size,stride:i.stride,offset:i.offset,startIndices:t,nested:a}),!1}return this.clearNeedsUpdate(),this.setData(e),!0}getVertexOffset(e){let{startIndices:t}=this;return(t?e<t.length?t[e]:this.numInstances:e)*this.size}getValue(){let e=this.settings.shaderAttributes,t=super.getValue();if(!e)return t;for(let n in e)Object.assign(t,super.getValue(n,e[n]));return t}getBufferLayout(e){this.state.layoutChanged=!1;let t=this.settings.shaderAttributes,n=super._getBufferLayout(),{stepMode:r}=this.settings;if(r===`dynamic`?n.stepMode=e?e.isInstanced?`instance`:`vertex`:`instance`:n.stepMode=r??`vertex`,!t)return n;for(let e in t){let r=super._getBufferLayout(e,t[e]);n.attributes.push(...r.attributes)}return n}_autoUpdater(e,{data:t,startRow:n,endRow:r,props:i,numInstances:a}){let{settings:o,state:s,value:c,size:l,startIndices:u}=e,{accessor:d,transform:f}=o,p=s.binaryAccessor||(typeof d==`function`?d:i[d]);H(typeof p==`function`,`accessor "${d}" is not a function`);let m=e.getVertexOffset(n),{iterable:h,objectInfo:g}=mh(t,n,r);for(let t of h){g.index++;let n=p(t,g);if(f&&(n=f.call(this,n)),u){let t=(g.index<u.length-1?u[g.index+1]:a)-u[g.index];if(n&&Array.isArray(n[0])){let t=m;for(let r of n)e._normalizeValue(r,c,t),t+=l}else n&&n.length>l?c.set(n,m):(e._normalizeValue(n,g.target,0),Ap({target:c,source:g.target,start:m,count:t}));m+=t*l}else e._normalizeValue(n,c,m),m+=l}}_validateAttributeUpdaters(){let{settings:e}=this;if(!(e.noAlloc||typeof e.update==`function`))throw Error(`Attribute ${this.id} missing update or accessor`)}_checkAttributeArray(){let{value:e}=this,t=Math.min(4,this.size);if(e&&e.length>=t){let n=!0;switch(t){case 4:n&&=Number.isFinite(e[3]);case 3:n&&=Number.isFinite(e[2]);case 2:n&&=Number.isFinite(e[1]);case 1:n&&=Number.isFinite(e[0]);break;default:n=!1}if(!n)throw Error(`Illegal attribute generated for ${this.id}`)}}},Ch=class e{gpuDataEvaluators;format;length;id;_gpuVector;_ownsGPUDataEvaluators;_destroyed=!1;static fromGPUVector(t){if(t.bufferLayout)throw Error(`GPUVectorEvaluator.fromGPUVector() does not accept interleaved vector "${t.name}"`);if(t.data.length===0)throw Error(`GPUVectorEvaluator.fromGPUVector() requires GPUData for "${t.name}"`);return new e({id:t.name,gpuDataEvaluators:t.data.map(e=>y.fromGPUData(e,{id:t.name})),gpuVector:t,format:t.format})}static fromGPUDataEvaluators(t,n={}){return new e({id:n.id,gpuDataEvaluators:t,format:n.format})}constructor({id:e,gpuDataEvaluators:t,gpuVector:n,format:r}){if(t.length===0)throw Error(`GPUVectorEvaluator requires at least one GPUData evaluator`);wh(t),this.id=e,this.gpuDataEvaluators=t,this.format=r??t[0].format,this.length=t.reduce((e,t)=>e+t.length,0),this._gpuVector=n,this._ownsGPUDataEvaluators=!n}get evaluated(){return!!this._gpuVector}get gpuVector(){if(!this._gpuVector)throw Error(`${this} not evaluated`);return this._gpuVector}mapGPUData(t){return e.fromGPUDataEvaluators(this.gpuDataEvaluators.map((e,n)=>t(e,n)),{id:this.id})}async evaluate(e,t={}){if(this._destroyed)throw Error(`GPUVectorEvaluator ${this} already destroyed`);if(this._gpuVector)return this._gpuVector;let n=await Promise.all(this.gpuDataEvaluators.map(n=>n.evaluate(e,t))),r=n[0],i=n.map(Th),a=t.format??this.format??r.format;return this._gpuVector=new m({type:`data`,name:t.name??this.id??`vector`,format:a,data:i,stride:r.stride,byteStride:r.byteStride,rowByteLength:r.rowByteLength,bufferLayout:r.bufferLayout}),this._gpuVector}evaluateSync(e,t={}){if(this._destroyed)throw Error(`GPUVectorEvaluator ${this} already destroyed`);if(this._gpuVector)return this._gpuVector;let n=this.gpuDataEvaluators.map(n=>n.evaluateSync(e,t)),r=n[0],i=n.map(Th),a=t.format??this.format??r.format;return this._gpuVector=new m({type:`data`,name:t.name??this.id??`vector`,format:a,data:i,stride:r.stride,byteStride:r.byteStride,rowByteLength:r.rowByteLength,bufferLayout:r.bufferLayout}),this._gpuVector}destroy(){if(this._ownsGPUDataEvaluators)for(let e of this.gpuDataEvaluators)e.destroy();this._gpuVector=void 0,this._destroyed=!0}toString(){return this.id??this.constructor.name}};function wh(e){let t=e[0];for(let n of e.slice(1))if(n.type!==t.type||n.size!==t.size||n.normalized!==t.normalized||n.format!==t.format)throw Error(`GPUVectorEvaluator requires matching GPUData evaluator layouts`)}function Th(e){let[t,...n]=e.data;if(!t||n.length>0)throw Error(`GPUVectorEvaluator requires one GPUData chunk for "${e.name}"`);return t}function Eh({elementWise:e,func:t,inputs:n,output:r,outputBuffer:i}){let a=Array.isArray(n)?n:Object.values(n);for(let e of a)if(!e.value)throw Error(`${e} does not have CPU value`);let o=r.length,s=r.size,c=new r.ValueType(o*s);for(let n=0;n<o;n++){let r=a.map(e=>Dh(e,n));if(e)for(let e=0;e<s;e++)c[n*s+e]=t.apply(null,r.map(t=>t[e]));else t.call(null,c.subarray(n*s,n*s+s),...r)}let l=r.ValueType.BYTES_PER_ELEMENT,u=r.offset/l,d=r.stride/l,f=s,p=c;if(u!==0||d!==f){p=new r.ValueType(u+r.byteLength/l);for(let e=0;e<o;e++){let t=e*f,n=u+e*d,r=c.subarray(t,t+s);p.set(r,n),i.write(r,n*l)}}else i.write(c);return{success:!0,value:p}}function Dh(e,t){let n=e.value,r=e.size,i=e.offset/e.ValueType.BYTES_PER_ELEMENT,a=e.stride/e.ValueType.BYTES_PER_ELEMENT,o=i+(e.isConstant?0:t)*a,s=n.slice(o,o+r);if(!e.normalized)return s;let c=new Float32Array(r);for(let t=0;t<r;t++)c[t]=Oh(s[t],e.type);return c}function Oh(e,t){switch(t){case`uint8`:return e/255;case`uint16`:return e/65535;case`uint32`:return e/4294967295;case`sint8`:return Math.max(e/127,-1);case`sint16`:return Math.max(e/32767,-1);case`sint32`:return Math.max(e/2147483647,-1);case`float32`:return e;default:throw Error(`Unsupported normalized source type ${t}`)}}var kh=({inputs:e,output:t,target:n})=>{for(let t of Object.values(e.namedInputs))if(!t.value)throw Error(`${t} does not have CPU value`);let r=new t.ValueType(t.length*t.size);for(let n=0;n<t.length;n++){let i=Object.fromEntries(Object.entries(e.namedInputs).map(([e,t])=>[e,Dh(t,n)]));for(let a=0;a<t.size;a++)r[n*t.size+a]=Ah(e.expression,i,a)}return n.write(r),{success:!0,value:r}};function Ah(e,t,n){switch(e.kind){case`input`:{let r=t[e.name];return n<r.length?r[n]:r.length===1?r[0]:0}case`literal`:return Array.isArray(e.value)?e.value[n]??0:e.value;case`call`:{jh(e.op,e.args.length);let r=e.args.map(e=>Ah(e,t,n));switch(e.op){case`add`:return r[0]+r[1];case`subtract`:return r[0]-r[1];case`multiply`:return r[0]*r[1];case`divide`:return r[0]/r[1];case`pow`:return r[0]**+r[1];case`sqrt`:return Math.sqrt(r[0]);case`abs`:return Math.abs(r[0]);case`sin`:return Math.sin(r[0]);case`cos`:return Math.cos(r[0]);case`tan`:return Math.tan(r[0]);case`exp`:return Math.exp(r[0]);case`log`:return Math.log(r[0]);default:{let t=e.op;throw Error(`Unsupported arithmetic op ${t}`)}}}default:throw Error(`Unsupported expression node ${e.kind}`)}}function jh(e,t){let n=l[e].arity;if(t!==n)throw Error(`Arithmetic op '${e}' expects ${n} args, got ${t}`)}var Mh=({inputs:e,output:t,target:n})=>{let{sourceValues:r}=e;if(!r.value)throw Error(`${r} does not have CPU value`);let i=new t.ValueType(t.length*t.size);if(r.length===0)return{success:!1,error:Error(`${r} is empty`)};for(let e=0;e<r.size;e++){let n=Dh(r,0)[e],a=e*t.size,o=a+1;i[a]=n,i[o]=n;for(let t=1;t<r.length;t++){let n=Dh(r,t)[e];n<i[a]&&(i[a]=n),n>i[o]&&(i[o]=n)}}return n.write(i),{success:!0,value:i}},Nh=({inputs:e,output:t,target:n})=>Eh({func:(e,t)=>{let n=e.length/2,r=new Float64Array(t.buffer);for(let t=0;t<n;t++){let i=r[t];e[t]=Math.fround(i),e[t+n]=i-e[t]}return e},inputs:e,output:t,outputBuffer:n}),Ph=async({inputs:e,output:t,target:n})=>{let{ids:r,sourceValues:i}=e,a=r.value,o=i.value;if(!a)throw Error(`${r} does not have CPU value`);if(!o)throw Error(`${i} does not have CPU value`);let s=new t.ValueType(t.length*t.size),c=Array(t.size).fill(0);for(let e=0;e<t.length;e++){let n=Dh(r,e),a=Number(n[0]),o=Fh(a,i.length)?Dh(i,a):c;s.set(o,e*t.size)}return n.write(s),{success:!0,value:s}};function Fh(e,t){return Number.isInteger(e)&&e>=0&&e<t}var Ih=({inputs:e,output:t,target:n})=>Eh({func:(e,...t)=>{let n=0;for(let r of t)e.set(r,n),n+=r.length},inputs:e,output:t,outputBuffer:n}),Lh=({inputs:e,output:t,target:n})=>{let{x:r,y:i}=e,a=new t.ValueType(t.length);for(let e=0;e<t.length;e++){let t=Dh(r,e),n=Dh(i,e),o=0;for(let e=0;e<r.size;e++)o+=t[e]*n[e];a[e]=o}return n.write(a),{success:!0,value:a}},Rh=({inputs:e,output:t,target:n})=>{let{x:r,y:i}=e,a=new t.ValueType(t.length);for(let e=0;e<t.length;e++){let t=Dh(r,e),n=Dh(i,e),o=1;for(let e=0;e<r.size;e++)if(t[e]!==n[e]){o=0;break}a[e]=o}return n.write(a),{success:!0,value:a}},zh=({inputs:e,output:t,target:n})=>{let{x:r}=e,i=new t.ValueType(t.length);for(let e=0;e<t.length;e++){let t=Dh(r,e),n=0;for(let e=0;e<r.size;e++)n+=t[e]*t[e];i[e]=Math.sqrt(n)}return n.write(i),{success:!0,value:i}},Bh=async({inputs:e,output:t,target:n})=>{let{segments:r,vertexCount:i}=e,a=r.value;if(!a)throw Error(`${r} does not have CPU value`);Vh(a,r,i);let o=new t.ValueType(t.length*t.size),s=0;for(let e=0;e<i;e++){for(;s+1<r.length&&a[Hh(r,s+1)]<=e;)s++;let n=a[Hh(r,s)],i=e*t.size;o[i]=s,o[i+1]=e-n}return n.write(o),{success:!0,value:o}};function Vh(e,t,n){if(t.length<1)throw Error(`segmentedMap segments must contain at least one segment start`);let r=0;for(let n=0;n<t.length;n++){let i=e[Hh(t,n)];if(n===0&&i!==0)throw Error(`segmentedMap segments must start at 0, got ${i}`);if(n>0&&i<r)throw Error(`segmentedMap segments must be non-decreasing, got ${i} after ${r}`);r=i}if(r>n)throw Error(`segmentedMap last segment start must be <= vertexCount, got ${r} > ${n}`)}function Hh(e,t){return e.offset/e.ValueType.BYTES_PER_ELEMENT+t*(e.stride/e.ValueType.BYTES_PER_ELEMENT)}var Uh=async({inputs:e,output:t,target:n})=>{let{condition:r,whenTrue:i,whenFalse:a}=e,o=new t.ValueType(t.length*t.size);for(let e=0;e<t.length;e++){let n=Dh(r,e),s=Dh(i,e),c=Dh(a,e);for(let l=0;l<t.size;l++){let u=Wh(n,r.size,l);o[e*t.size+l]=u===0?Wh(c,a.size,l):Wh(s,i.size,l)}}return n.write(o),{success:!0,value:o}};function Wh(e,t,n){return n<t?e[n]:t===1?e[0]:0}var Gh=({inputs:e,output:t,target:n})=>{let r=new t.ValueType(t.length);for(let n=0;n<t.length;n++)r[n]=e.start+n*e.step;return n.write(r),{success:!0,value:r}},Kh=({inputs:e,output:t,target:n})=>{let{columns:r}=e;return Eh({func:(e,t)=>{for(let n=0;n<r.length;n++)e[n]=t[r[n]]},inputs:{x:e.x},output:t,outputBuffer:n})},qh=e({arithmetic:()=>kh,dot:()=>Lh,equalAll:()=>Rh,extent:()=>Mh,fround:()=>Nh,gather:()=>Ph,interleave:()=>Ih,length:()=>zh,segmentedMap:()=>Bh,select:()=>Uh,sequence:()=>Gh,swizzle:()=>Kh}),Jh=new class{_modules={cpu:qh};add(e,t){let n=this._modules[e];if(typeof t.then==`function`){let r=Promise.all([Promise.resolve(n||{}),t]).then(([e,t])=>({...e,...t}));return this._modules[e]=r,r.then(t=>{this._modules[e]=t}).catch(t=>{w.error(`Failed to register ${e} backend: ${t}`)()}),r}if(n&&typeof n.then==`function`){let r=Promise.resolve(n).then(e=>({...e,...t})).then(t=>(this._modules[e]=t,t)).catch(t=>{throw w.error(`Failed to register ${e} backend: ${t}`)(),t});return this._modules[e]=r,r}let r={...n||{},...t};return this._modules[e]=r,Promise.resolve(r)}async get(e,t){let n=this._modules[e];if(!n)if(e===`webgl`)n=this.add(`webgl`,o(()=>import(`./webgl-Rrlc2Lyq.js`),__vite__mapDeps([4,5,2,3,6]),import.meta.url));else if(e===`webgpu`)n=this.add(`webgpu`,o(()=>import(`./webgpu-yD2IvmEH.js`).then(e=>e.t),__vite__mapDeps([7,1,5,2]),import.meta.url));else throw Error(`${e} backend not registered`);let r=(await n)[t];if(typeof r!=`function`)throw Error(`${e} backend does not implement ${t}`);return r}getSync(e,t){let n=this._modules[e];if(!n)throw Error(`${e} backend not registered`);if(typeof n.then==`function`)throw Error(`${e} backend is not loaded yet`);let r=n[t];if(typeof r!=`function`)throw Error(`${e} backend does not implement ${t}`);return r}clear(){this._modules={}}},Yh=class{inputs;dependencies;constructor(e){this.inputs=e,this.dependencies=Array.from(e instanceof Array?e:Object.values(e)).filter(e=>e instanceof y)}async execute(e,t){return await this._resolveDependencies(e),await this._executeWithHandler(await Jh.get(this._getHandlerRegistry(e),this.name),t)}executeSync(e,t){this._resolveDependenciesSync(e);let n=this._executeWithHandler(Jh.getSync(this._getHandlerRegistry(e),this.name),t);if(Xh(n))throw Error(`${this.name} returned a Promise in executeSync()`);return n}shouldExecuteOnCPU(){return this.output.length<=1&&Array.from(this.dependencies).every(e=>!!e.value)}_getHandlerRegistry(e){return this.shouldExecuteOnCPU()?`cpu`:e.type}async _resolveDependencies(e){for(let t of this.dependencies)await t.evaluate(e);if(this._getHandlerRegistry(e)===`cpu`||e.type===`null`)for(let e of this.dependencies)await e.ensureCPUValue()}_resolveDependenciesSync(e){for(let t of this.dependencies)t.evaluateSync(e);if(this._getHandlerRegistry(e)===`cpu`||e.type===`null`)for(let e of this.dependencies)e.ensureCPUValueSync()}_executeWithHandler(e,t){return e({device:t.device,inputs:this.inputs,output:this.output,target:t})}};function Xh(e){return typeof e?.then==`function`}function Zh(...e){let t=Qh(e.map(e=>e.type));return t[0]!==`f`&&e.some(e=>e.normalized)&&(t=`float32`),{isConstant:e.every(e=>e.isConstant),type:t,size:e.reduce((e,t)=>Math.max(e,t.size),0),length:e.reduce((e,t)=>Math.max(e,t.length),0)}}function Qh(e){let t=0,n=0;for(let r of e){if(r[0]===`f`)return`float32`;let e=r.endsWith(`8`)?8:r.endsWith(`6`)?16:32;r[0]===`u`?t=Math.max(t,e):n=Math.max(n,e)}return t&&!n?`uint${t}`:n&&t<32?`sint${Math.max(n,t*2)}`:`float32`}var $h=class extends Yh{name=`interleave`;output;constructor(e){super(e);let{isConstant:t,type:n,length:r}=Zh(...e);this.output=new y({isConstant:t,type:n,size:e.reduce((e,t)=>e+t.size,0),length:r,source:this})}toString(){return`_${this.inputs.join(`_`)}_`}};function eg(...e){if(e.length===0)throw Error(`interleave() requires at least one input`);return e.length===1?g(e[0]):new $h(e.map(g)).output}function tg(e,t){let n=rg(t);for(let t of n)t.evaluateSync(e);return ng(n),t}function ng(e){let t=new Set(e.flatMap(sg)),n=new Set;for(let t of e)og(t,n);for(let e of n)e.evaluated&&!t.has(e.buffer)&&e.destroy()}function rg(e){let t=new Set;return ig(e,t,new Set),Array.from(t)}function ig(e,t,n){if(cg(e)){t.add(e);return}if(!(!e||typeof e!=`object`||n.has(e))){if(n.add(e),Array.isArray(e)){for(let r of e)ig(r,t,n);return}if(ag(e))for(let r of Object.values(e))ig(r,t,n)}}function ag(e){let t=Object.getPrototypeOf(e);return t===Object.prototype||t===null}function og(e,t){if(e instanceof Ch){for(let n of e.gpuDataEvaluators)og(n,t);return}let n=e.source;if(n){if(n instanceof y){t.has(n)||(t.add(n),og(n,t));return}for(let e of n.dependencies)t.has(e)||(t.add(e),og(e,t))}}function sg(e){return e instanceof y?[e.buffer]:e.gpuVector.data.map(e=>e.buffer instanceof u?e.buffer.buffer:e.buffer)}function cg(e){return e instanceof y||e instanceof Ch}var lg=class{constructor(e,{id:t,isTransitionAttribute:n}){this.packedBuffers={},this.device=e,this.id=t,this.isTransitionAttribute=n,this.device.type===`webgpu`&&Jh.add(`webgpu`,{interleave:he})}hasGroups(e){return this.device.type===`webgpu`&&Object.values(e).some(e=>!!e.settings.bufferGroup)}finalize(){for(let e of Object.values(this.packedBuffers))e.packed.destroy();this.packedBuffers={}}getBufferLayouts(e,t){let n=this._getPackedGroups(e,t,{requireValues:!1,excludeAttributes:{}});return this._getBufferLayouts(e,n,t)}getBindings(e,t,n,r){let i=this._getPackedGroups(e,n,{requireValues:!0,excludeAttributes:r}),a={},o=new Set;for(let e of i.values()){let n=!this.packedBuffers[e.id]||e.attributes.some(e=>!!t[e.id]);a[e.id]=this._getPackedBuffer(e,n);for(let t of e.attributes)o.add(t.id)}return{bufferLayouts:this._getBufferLayouts(e,i,n).filter(t=>!r[t.name]&&!e[t.name]?.settings.isIndexed),buffers:a,groupedAttributeIds:o}}_getPackedGroups(e,t,{requireValues:n,excludeAttributes:r}){let i=new Map;for(let t of Object.values(e)){let e=t.settings.bufferGroup;if(!e)continue;let n=i.get(e)||[];n.push(t),i.set(e,n)}let a=new Map;for(let[e,o]of i){let i=this._getPackedGroup(e,o,t,n,r);i&&a.set(e,i)}return a}_getPackedGroup(e,t,n,r,i){if(t.length<2)return null;let a=t.map(e=>e.getBufferLayout(n)),o=a[0].stepMode,s=Math.max(1,t[0].numInstances),c=r&&t.every(e=>e.isConstant);for(let e=0;e<t.length;e++){let n=t[e],c=n.getAccessor(),l=c.size*c.bytesPerElement;if(i[n.id]||n.settings.isIndexed||n.settings.noAlloc||n.doublePrecision||this.isTransitionAttribute(n.id)||a[e].stepMode!==o||n.numInstances!==t[0].numInstances||(c.offset||0)!==0||(c.vertexOffset||0)!==0||sh(c)!==l||r&&(n.isConstant?!n.getConstantValue()||n.getConstantValue().byteLength<l:!ArrayBuffer.isView(n.value)||n.value.byteLength<s*l))return null}let l={},u=[],d=0;for(let e=0;e<t.length;e++){let n=t[e];d=ug(d),l[n.id]=d;for(let t of a[e].attributes||[])u.push({...t,byteOffset:d+(t.byteOffset||0)});d+=sh(n.getAccessor())}return d=ug(d),{id:e,attributes:t,byteStride:d,byteOffsets:l,rowCount:s,layout:{name:e,byteStride:c?0:d,stepMode:o,attributes:u}}}_getBufferLayouts(e,t,n){let r=[],i=new Set,a=new Set;for(let e of t.values())for(let t of e.attributes)a.add(t.id);for(let o of Object.values(e)){let e=o.settings.bufferGroup,s=e&&t.get(e);s&&a.has(o.id)?i.has(s.id)||(r.push(s.layout),i.add(s.id)):r.push(o.getBufferLayout(n))}return r}_getPackedBuffer(e,t){let n=JSON.stringify({byteStride:e.layout.byteStride,attributes:e.layout.attributes}),r=this.packedBuffers[e.id];if((!r||r.layoutKey!==n)&&(t=!0),t){r&&(r.packed.destroy(),delete this.packedBuffers[e.id]);let t=this._interleavePackedGroup(e);return this.packedBuffers[e.id]={packed:t,layoutKey:n},t.buffer}if(!r)throw Error(`Attribute buffer group ${e.id} has no packed buffer`);return r.packed.buffer}_interleavePackedGroup(e){let t=eg(...e.attributes.map(t=>this._getInterleaveInput(e,t)));return tg(this.device,t),t}_getInterleaveInput(e,t){let n=sh(t.getAccessor()),r=e.byteOffsets[t.id];if(dg(`${e.id}.${t.id} rowByteLength`,n),dg(`${e.id}.${t.id} groupByteOffset`,r),t.isConstant){let r=t.getConstantValue();if(!r)throw Error(`Attribute group ${e.id} is missing constant value ${t.id}`);return dg(`${e.id}.${t.id} constant byteOffset`,r.byteOffset),new y({id:t.id,type:`uint32`,size:n/4,isConstant:!0,value:new Uint32Array(r.buffer,r.byteOffset,n/Uint32Array.BYTES_PER_ELEMENT)})}let i=t.getBuffer(),a=t.byteOffset,o=t.getAccessor().stride||n;if(dg(`${e.id}.${t.id} byteOffset`,a),dg(`${e.id}.${t.id} stride`,o),!i)throw Error(`Attribute group ${e.id} cannot interleave missing buffer ${t.id}`);return new y({id:t.id,type:`uint32`,size:n/4,offset:a,stride:o,length:e.rowCount,buffer:i})}};function ug(e){return Math.ceil(e/4)*4}function dg(e,t){if(t%4!=0)throw Error(`Attribute buffer groups require 32-bit alignment: ${e}=${t}`)}function fg(e){let{source:t,target:n,start:r=0,size:i,getData:a}=e,o=e.end||n.length,s=t.length,c=o-r;if(s>c){n.set(t.subarray(0,c),r);return}if(n.set(t,r),!a)return;let l=s;for(;l<c;){let e=a(l,t);for(let t=0;t<i;t++)n[r+l]=e[t]||0,l++}}function pg({source:e,target:t,size:n,getData:r,sourceStartIndices:i,targetStartIndices:a}){if(!i||!a)return fg({source:e,target:t,size:n,getData:r}),t;let o=0,s=0,c=r&&((e,t)=>r(e+s,t)),l=Math.min(i.length,a.length);for(let r=1;r<l;r++){let l=i[r]*n,u=a[r]*n;fg({source:e.subarray(o,l),target:t,start:s,end:u,size:n,getData:c}),o=l,s=u}return s<t.length&&fg({source:[],target:t,start:s,size:n,getData:c}),t}function mg(e){let{device:t,settings:n,value:r}=e,i=new Sh(t,n);return i.setData({value:r instanceof Float64Array?new Float64Array:new Float32Array,normalized:n.normalized}),i}function hg(e){switch(e){case 1:return`float`;case 2:return`vec2`;case 3:return`vec3`;case 4:return`vec4`;default:throw Error(`No defined attribute type for size "${e}"`)}}function gg(e){switch(e){case 1:return`float32`;case 2:return`float32x2`;case 3:return`float32x3`;case 4:return`float32x4`;default:throw Error(`invalid type size`)}}function _g(e){e.push(e.shift())}function vg(e,t){let{settings:n,value:r,size:i}=e,a=e.isDoublePrecisionBuffer?2:1,o=0,{shaderAttributes:s}=e.settings;if(s)for(let e of Object.values(s))o=Math.max(o,e.vertexOffset??0);return(n.noAlloc?r.length:(t+o)*i)*a}function yg({device:e,source:t,target:n}){return(!n||n.byteLength<t.byteLength)&&(n?.destroy(),n=e.createBuffer({byteLength:t.byteLength,usage:t.usage})),n}function bg({device:e,buffer:t,attribute:n,fromLength:r,toLength:i,fromStartIndices:a,getData:o=e=>e}){let s=n.isDoublePrecisionBuffer?2:1,c=n.size*s,l=n.byteOffset,u=n.settings.bytesPerElement<4?l/n.settings.bytesPerElement*4:l,d=n.startIndices,f=a&&d,p=n.isConstant;if(!f&&t&&r>=i)return t;let m=n.value instanceof Float64Array?Float32Array:n.value.constructor,h=p?n.value:new m(n.getBuffer().readSyncWebGL(l,i*m.BYTES_PER_ELEMENT).buffer);if(n.settings.normalized&&!p){let e=o;o=(t,r)=>n.normalizeConstant(e(t,r))}let g=p?(e,t)=>o(h,t):(e,t)=>o(h.subarray(e+l,e+l+c),t),_=t?new Float32Array(t.readSyncWebGL(u,r*4).buffer):new Float32Array,v=new Float32Array(i);return pg({source:_,target:v,sourceStartIndices:a,targetStartIndices:d,size:c,getData:g}),(!t||t.byteLength<v.byteLength+u)&&(t?.destroy(),t=e.createBuffer({byteLength:v.byteLength+u,usage:35050})),t.write(v,u),t}var xg=class{constructor({device:e,attribute:t,timeline:n}){this.buffers=[],this.currentLength=0,this.device=e,this.transition=new Zp(n),this.attribute=t,this.attributeInTransition=mg(t),this.currentStartIndices=t.startIndices}get inProgress(){return this.transition.inProgress}start(e,t,n=1/0){this.settings=e,this.currentStartIndices=this.attribute.startIndices,this.currentLength=vg(this.attribute,t),this.transition.start({...e,duration:n})}update(){let e=this.transition.update();return e&&this.onUpdate(),e}setBuffer(e){let{stride:t}=this.attributeInTransition.getAccessor();this.attributeInTransition.setData({buffer:e,normalized:this.attribute.settings.normalized,value:this.attributeInTransition.value,stride:t})}cancel(){this.transition.cancel()}delete(){this.cancel();for(let e of this.buffers)e.destroy();this.buffers.length=0}},Sg=class extends xg{constructor({device:e,attribute:t,timeline:n}){super({device:e,attribute:t,timeline:n}),this.type=`interpolation`,this.transform=Dg(e,t)}start(e,t){let n=this.currentLength,r=this.currentStartIndices;if(super.start(e,t,e.duration),e.duration<=0){this.transition.cancel();return}let{buffers:i,attribute:a}=this;_g(i),i[0]=bg({device:this.device,buffer:i[0],attribute:a,fromLength:n,toLength:this.currentLength,fromStartIndices:r,getData:e.enter}),i[1]=yg({device:this.device,source:i[0],target:i[1]}),this.setBuffer(i[1]);let{transform:o}=this,s=o.model,c=Math.floor(this.currentLength/a.size);Eg(a)&&(c/=2),s.setVertexCount(c),a.isConstant?(s.setAttributes({aFrom:i[0]}),s.setConstantAttributes({aTo:a.value})):s.setAttributes({aFrom:i[0],aTo:a.getBuffer()}),o.transformFeedback.setBuffers({vCurrent:i[1]})}onUpdate(){let{duration:e,easing:t}=this.settings,{time:n}=this.transition,r=n/e;t&&(r=t(r));let{model:i}=this.transform,a={time:r};i.shaderInputs.setProps({interpolation:a}),this.transform.run({discard:!0})}delete(){super.delete(),this.transform.destroy()}},Cg={name:`interpolation`,vs:`layout(std140) uniform interpolationUniforms {
  float time;
} interpolation;
`,uniformTypes:{time:`f32`}},wg=`#version 300 es
#define SHADER_NAME interpolation-transition-vertex-shader

in ATTRIBUTE_TYPE aFrom;
in ATTRIBUTE_TYPE aTo;
out ATTRIBUTE_TYPE vCurrent;

void main(void) {
  vCurrent = mix(aFrom, aTo, interpolation.time);
  gl_Position = vec4(0.0);
}
`,Tg=`#version 300 es
#define SHADER_NAME interpolation-transition-vertex-shader

in ATTRIBUTE_TYPE aFrom;
in ATTRIBUTE_TYPE aFrom64Low;
in ATTRIBUTE_TYPE aTo;
in ATTRIBUTE_TYPE aTo64Low;
out ATTRIBUTE_TYPE vCurrent;
out ATTRIBUTE_TYPE vCurrent64Low;

vec2 mix_fp64(vec2 a, vec2 b, float x) {
  vec2 range = sub_fp64(b, a);
  return sum_fp64(a, mul_fp64(range, vec2(x, 0.0)));
}

void main(void) {
  for (int i=0; i<ATTRIBUTE_SIZE; i++) {
    vec2 value = mix_fp64(vec2(aFrom[i], aFrom64Low[i]), vec2(aTo[i], aTo64Low[i]), interpolation.time);
    vCurrent[i] = value.x;
    vCurrent64Low[i] = value.y;
  }
  gl_Position = vec4(0.0);
}
`;function Eg(e){return e.isDoublePrecisionBuffer}function Dg(e,t){let n=t.size,r=hg(n),i=gg(n),a=t.getBufferLayout();return Eg(t)?new me(e,{vs:Tg,bufferLayout:[{name:`aFrom`,byteStride:8*n,attributes:[{attribute:`aFrom`,format:i,byteOffset:0},{attribute:`aFrom64Low`,format:i,byteOffset:4*n}]},{name:`aTo`,byteStride:8*n,attributes:[{attribute:`aTo`,format:i,byteOffset:0},{attribute:`aTo64Low`,format:i,byteOffset:4*n}]}],modules:[pr,Cg],defines:{ATTRIBUTE_TYPE:r,ATTRIBUTE_SIZE:n},moduleSettings:{},varyings:[`vCurrent`,`vCurrent64Low`],bufferMode:35980,disableWarnings:!0}):new me(e,{vs:wg,bufferLayout:[{name:`aFrom`,format:i},{name:`aTo`,format:a.attributes[0].format}],modules:[Cg],defines:{ATTRIBUTE_TYPE:r},varyings:[`vCurrent`],disableWarnings:!0})}var Og=class extends xg{constructor({device:e,attribute:t,timeline:n}){super({device:e,attribute:t,timeline:n}),this.type=`spring`,this.texture=Ng(e),this.framebuffer=Pg(e,this.texture),this.transform=Mg(e,t)}start(e,t){let n=this.currentLength,r=this.currentStartIndices;super.start(e,t);let{buffers:i,attribute:a}=this;for(let t=0;t<2;t++)i[t]=bg({device:this.device,buffer:i[t],attribute:a,fromLength:n,toLength:this.currentLength,fromStartIndices:r,getData:e.enter});i[2]=yg({device:this.device,source:i[0],target:i[2]}),this.setBuffer(i[1]);let{model:o}=this.transform;o.setVertexCount(Math.floor(this.currentLength/a.size)),a.isConstant?o.setConstantAttributes({aTo:a.value}):o.setAttributes({aTo:a.getBuffer()})}onUpdate(){let{buffers:e,transform:t,framebuffer:n,transition:r}=this,i=this.settings;t.model.setAttributes({aPrev:e[0],aCur:e[1]}),t.transformFeedback.setBuffers({vNext:e[2]});let a={stiffness:i.stiffness,damping:i.damping};t.model.shaderInputs.setProps({spring:a}),t.run({framebuffer:n,discard:!1,parameters:{viewport:[0,0,1,1]},clearColor:[0,0,0,0]}),_g(e),this.setBuffer(e[1]),this.device.readPixelsToArrayWebGL(n)[0]>0||r.end()}delete(){super.delete(),this.transform.destroy(),this.texture.destroy(),this.framebuffer.destroy()}},kg={name:`spring`,vs:`layout(std140) uniform springUniforms {
  float damping;
  float stiffness;
} spring;
`,uniformTypes:{damping:`f32`,stiffness:`f32`}},Ag=`#version 300 es
#define SHADER_NAME spring-transition-vertex-shader

#define EPSILON 0.00001

in ATTRIBUTE_TYPE aPrev;
in ATTRIBUTE_TYPE aCur;
in ATTRIBUTE_TYPE aTo;
out ATTRIBUTE_TYPE vNext;
out float vIsTransitioningFlag;

ATTRIBUTE_TYPE getNextValue(ATTRIBUTE_TYPE cur, ATTRIBUTE_TYPE prev, ATTRIBUTE_TYPE dest) {
  ATTRIBUTE_TYPE velocity = cur - prev;
  ATTRIBUTE_TYPE delta = dest - cur;
  ATTRIBUTE_TYPE force = delta * spring.stiffness;
  ATTRIBUTE_TYPE resistance = velocity * spring.damping;
  return force - resistance + velocity + cur;
}

void main(void) {
  bool isTransitioning = length(aCur - aPrev) > EPSILON || length(aTo - aCur) > EPSILON;
  vIsTransitioningFlag = isTransitioning ? 1.0 : 0.0;

  vNext = getNextValue(aCur, aPrev, aTo);
  gl_Position = vec4(0, 0, 0, 1);
  gl_PointSize = 100.0;
}
`,jg=`#version 300 es
#define SHADER_NAME spring-transition-is-transitioning-fragment-shader

in float vIsTransitioningFlag;

out vec4 fragColor;

void main(void) {
  if (vIsTransitioningFlag == 0.0) {
    discard;
  }
  fragColor = vec4(1.0);
}`;function Mg(e,t){let n=hg(t.size),r=gg(t.size);return new me(e,{vs:Ag,fs:jg,bufferLayout:[{name:`aPrev`,format:r},{name:`aCur`,format:r},{name:`aTo`,format:t.getBufferLayout().attributes[0].format}],varyings:[`vNext`],modules:[kg],defines:{ATTRIBUTE_TYPE:n},parameters:{depthCompare:`always`,blendColorOperation:`max`,blendColorSrcFactor:`one`,blendColorDstFactor:`one`,blendAlphaOperation:`max`,blendAlphaSrcFactor:`one`,blendAlphaDstFactor:`one`}})}function Ng(e){return e.createTexture({data:new Uint8Array(4),format:`rgba8unorm`,width:1,height:1})}function Pg(e,t){return e.createFramebuffer({id:`spring-transition-is-transitioning-framebuffer`,width:1,height:1,colorAttachments:[t]})}var Fg={interpolation:Sg,spring:Og},Ig=class{constructor(e,{id:t,timeline:n}){if(!e)throw Error(`AttributeTransitionManager is constructed without device`);this.id=t,this.device=e,this.timeline=n,this.transitions={},this.needsRedraw=!1,this.numInstances=1}finalize(){for(let e in this.transitions)this._removeTransition(e)}update({attributes:e,transitions:t,numInstances:n}){this.numInstances=n||1;for(let n in e){let r=e[n],i=r.getTransitionSetting(t);i&&this._updateAttribute(n,r,i)}for(let n in this.transitions){let r=e[n];(!r||!r.getTransitionSetting(t))&&this._removeTransition(n)}}hasAttribute(e){let t=this.transitions[e];return t&&t.inProgress}getAttributes(){let e={};for(let t in this.transitions){let n=this.transitions[t];n.inProgress&&(e[t]=n.attributeInTransition)}return e}run(){if(this.numInstances===0)return!1;for(let e in this.transitions)this.transitions[e].update()&&(this.needsRedraw=!0);let e=this.needsRedraw;return this.needsRedraw=!1,e}_removeTransition(e){this.transitions[e].delete(),delete this.transitions[e]}_updateAttribute(e,t,n){let r=this.transitions[e],i=!r||r.type!==n.type;if(i){r&&this._removeTransition(e);let a=Fg[n.type];a?this.transitions[e]=new a({attribute:t,timeline:this.timeline,device:this.device}):(z.error(`unsupported transition type '${n.type}'`)(),i=!1)}(i||t.needsRedraw())&&(this.needsRedraw=!0,this.transitions[e].start(n,this.numInstances))}},Lg=`attributeManager.invalidate`,Rg=`attributeManager.updateStart`,zg=`attributeManager.updateEnd`,Bg=`attribute.updateStart`,Vg=`attribute.allocate`,Hg=`attribute.updateEnd`,Ug=class{constructor(e,{id:t=`attribute-manager`,stats:n,timeline:r}={}){this.mergeBoundsMemoized=od(ip),this.id=t,this.device=e,this.attributes={},this.updateTriggers={},this.needsRedraw=!0,this.userData={},this.stats=n,this.attributeTransitionManager=new Ig(e,{id:`${t}-transitions`,timeline:r}),this.attributeBufferGroups=e.type===`webgpu`?new lg(e,{id:t,isTransitionAttribute:e=>this.attributeTransitionManager.hasAttribute(e)}):null,Object.seal(this)}finalize(){this.attributeBufferGroups?.finalize();for(let e in this.attributes)this.attributes[e].delete();this.attributeTransitionManager.finalize()}getNeedsRedraw(e={clearRedrawFlags:!1}){let t=this.needsRedraw;return this.needsRedraw=this.needsRedraw&&!e.clearRedrawFlags,t&&this.id}setNeedsRedraw(){this.needsRedraw=!0}add(e){this._add(e)}addInstanced(e){this._add(e,{stepMode:`instance`})}remove(e){for(let t of e)this.attributes[t]!==void 0&&(this.attributes[t].delete(),delete this.attributes[t])}invalidate(e,t){let n=this._invalidateTrigger(e,t);el(Lg,this,e,n)}invalidateAll(e){for(let t in this.attributes)this.attributes[t].setNeedsUpdate(t,e);el(Lg,this,`all`)}update({data:e,numInstances:t,startIndices:n=null,transitions:r,props:i={},buffers:a={},context:o={}}){let s=!1;el(Rg,this),this.stats&&this.stats.get(`Update Attributes`).timeStart();for(let r in this.attributes){let c=this.attributes[r],l=c.settings.accessor;c.startIndices=n,c.numInstances=t,i[r]&&z.removed(`props.${r}`,`data.attributes.${r}`)(),c.setExternalBuffer(a[r])||c.setBinaryValue(typeof l==`string`?a[l]:void 0,e.startIndices)||typeof l==`string`&&!a[l]&&c.setConstantValue(o,i[l])||c.needsUpdate()&&(s=!0,this._updateAttribute({attribute:c,numInstances:t,data:e,props:i,context:o})),this.needsRedraw=this.needsRedraw||c.needsRedraw()}s&&el(zg,this,t),this.stats&&(this.stats.get(`Update Attributes`).timeEnd(),s&&this.stats.get(`Attributes updated`).incrementCount()),this.attributeTransitionManager.update({attributes:this.attributes,numInstances:t,transitions:r})}updateTransition(){let{attributeTransitionManager:e}=this,t=e.run();return this.needsRedraw=this.needsRedraw||t,t}getAttributes(){return{...this.attributes,...this.attributeTransitionManager.getAttributes()}}getBounds(e){let t=e.map(e=>this.attributes[e]?.getBounds());return this.mergeBoundsMemoized(t)}getChangedAttributes(e={clearChangedFlags:!1}){let{attributes:t,attributeTransitionManager:n}=this,r={...n.getAttributes()};for(let i in t){let a=t[i];a.needsRedraw(e)&&!n.hasAttribute(i)&&(r[i]=a)}return r}getBufferLayouts(e){return this.hasBufferGroups()?this.attributeBufferGroups.getBufferLayouts(this.getAttributes(),e):Object.values(this.getAttributes()).map(t=>t.getBufferLayout(e))}hasBufferGroups(){return!!this.attributeBufferGroups?.hasGroups(this.attributes)}getBufferGroupBindings(e,t,n={}){return this.attributeBufferGroups?this.attributeBufferGroups.getBindings(this.getAttributes(),e,t,n):{bufferLayouts:this.getBufferLayouts(t),buffers:{},groupedAttributeIds:new Set}}_add(e,t){for(let n in e){let r=e[n],i={...r,id:n,size:r.isIndexed&&1||r.size||1,...t};this.attributes[n]=new Sh(this.device,i)}this._mapUpdateTriggersToAttributes()}_mapUpdateTriggersToAttributes(){let e={};for(let t in this.attributes)this.attributes[t].getUpdateTriggers().forEach(n=>{e[n]||(e[n]=[]),e[n].push(t)});this.updateTriggers=e}_invalidateTrigger(e,t){let{attributes:n,updateTriggers:r}=this,i=r[e];return i&&i.forEach(e=>{let r=n[e];r&&r.setNeedsUpdate(r.id,t)}),i}_updateAttribute(e){let{attribute:t,numInstances:n}=e;if(el(Bg,t),t.constant){t.setConstantValue(e.context,t.value);return}t.allocate(n)&&el(Vg,t,n),t.updateBuffer(e)&&(this.needsRedraw=!0,el(Hg,t,n))}},Wg=class extends Zp{get value(){return this._value}_onUpdate(){let{time:e,settings:{fromValue:t,toValue:n,duration:r,easing:i}}=this,a=i(e/r);this._value=Ne(t,n,a)}},Gg=1e-5;function Kg(e,t,n,r,i){let a=t-e;return(n-t)*i+-a*r+a+t}function qg(e,t,n,r,i){if(Array.isArray(n)){let a=[];for(let o=0;o<n.length;o++)a[o]=Kg(e[o],t[o],n[o],r,i);return a}return Kg(e,t,n,r,i)}function Jg(e,t){if(Array.isArray(e)){let n=0;for(let r=0;r<e.length;r++){let i=e[r]-t[r];n+=i*i}return Math.sqrt(n)}return Math.abs(e-t)}var Yg={interpolation:Wg,spring:class extends Zp{get value(){return this._currValue}_onUpdate(){let{fromValue:e,toValue:t,damping:n,stiffness:r}=this.settings,{_prevValue:i=e,_currValue:a=e}=this,o=qg(i,a,t,n,r),s=Jg(o,t),c=Jg(o,a);s<Gg&&c<Gg&&(o=t,this.end()),this._prevValue=a,this._currValue=o}}},Xg=class{constructor(e){this.transitions=new Map,this.timeline=e}get active(){return this.transitions.size>0}add(e,t,n,r){let{transitions:i}=this;if(i.has(e)){let n=i.get(e),{value:r=n.settings.fromValue}=n;t=r,this.remove(e)}if(r=xh(r),!r)return;let a=Yg[r.type];if(!a){z.error(`unsupported transition type '${r.type}'`)();return}let o=new a(this.timeline);o.start({...r,fromValue:t,toValue:n}),i.set(e,o)}remove(e){let{transitions:t}=this;t.has(e)&&(t.get(e).cancel(),t.delete(e))}update(){let e={};for(let[t,n]of this.transitions)n.update(),e[t]=n.value,n.inProgress||this.remove(t);return e}clear(){for(let e of this.transitions.keys())this.remove(e)}};function Zg(e){let t=e[Cp];for(let n in t){let r=t[n],{validate:i}=r;if(i&&!i(e[n],r))throw Error(`Invalid prop ${n}: ${e[n]}`)}}function Qg(e,t){let n=e_({newProps:e,oldProps:t,propTypes:e[Cp],ignoreProps:{data:null,updateTriggers:null,extensions:null,transitions:null}}),r=n_(e,t),i=!1;return r||(i=r_(e,t)),{dataChanged:r,propsChanged:n,updateTriggersChanged:i,extensionsChanged:i_(e,t),transitionsChanged:$g(e,t)}}function $g(e,t){if(!e.transitions)return!1;let n={},r=e[Cp],i=!1;for(let a in e.transitions){let o=r[a],s=o&&o.type;(s===`number`||s===`color`||s===`array`)&&t_(e[a],t[a],o)&&(n[a]=!0,i=!0)}return i?n:!1}function e_({newProps:e,oldProps:t,ignoreProps:n={},propTypes:r={},triggerName:i=`props`}){if(t===e)return!1;if(typeof e!=`object`||!e||typeof t!=`object`||!t)return`${i} changed shallowly`;for(let a of Object.keys(e))if(!(a in n)){if(!(a in t))return`${i}.${a} added`;let n=t_(e[a],t[a],r[a]);if(n)return`${i}.${a} ${n}`}for(let a of Object.keys(t))if(!(a in n)){if(!(a in e))return`${i}.${a} dropped`;if(!Object.hasOwnProperty.call(e,a)){let n=t_(e[a],t[a],r[a]);if(n)return`${i}.${a} ${n}`}}return!1}function t_(e,t,n){let r=n&&n.equal;return r&&!r(e,t,n)||!r&&(r=e&&t&&e.equals,r&&!r.call(e,t))?`changed deeply`:!r&&t!==e?`changed shallowly`:null}function n_(e,t){if(t===null)return`oldProps is null, initial diff`;let n=!1,{dataComparator:r,_dataDiff:i}=e;return r?r(e.data,t.data)||(n=`Data comparator detected a change`):e.data!==t.data&&(n=`A new data container was supplied`),n&&i&&(n=i(e.data,t.data)||n),n}function r_(e,t){if(t===null||`all`in e.updateTriggers&&a_(e,t,`all`))return{all:!0};let n={},r=!1;for(let i in e.updateTriggers)i!==`all`&&a_(e,t,i)&&(n[i]=!0,r=!0);return r?n:!1}function i_(e,t){if(t===null)return!0;let n=t.extensions,{extensions:r}=e;if(r===n)return!1;if(!n||!r||r.length!==n.length)return!0;for(let e=0;e<r.length;e++)if(!r[e].equals(n[e]))return!0;return!1}function a_(e,t,n){let r=e.updateTriggers[n];r??={};let i=t.updateTriggers[n];return i??={},e_({oldProps:i,newProps:r,triggerName:n})}var o_=`count(): argument not an object`,s_=`count(): argument not a container`;function c_(e){if(!u_(e))throw Error(o_);if(typeof e.count==`function`)return e.count();if(Number.isFinite(e.size))return e.size;if(Number.isFinite(e.length))return e.length;if(l_(e))return Object.keys(e).length;throw Error(s_)}function l_(e){return typeof e==`object`&&!!e&&e.constructor===Object}function u_(e){return typeof e==`object`&&!!e}function d_(e,t){if(!t)return e;let n={...e,...t};if(`defines`in t&&(n.defines={...e.defines,...t.defines}),`modules`in t&&(n.modules=(e.modules||[]).concat(t.modules),t.modules.some(e=>e.name===`project64`))){let e=n.modules.findIndex(e=>e.name===`project32`);e>=0&&n.modules.splice(e,1)}if(`inject`in t)if(!e.inject)n.inject=t.inject;else{let r={...e.inject};for(let e in t.inject)r[e]=(r[e]||``)+t.inject[e];n.inject=r}return n}var f_={minFilter:`linear`,mipmapFilter:`linear`,magFilter:`linear`,addressModeU:`clamp-to-edge`,addressModeV:`clamp-to-edge`},p_={};function m_(e,t,n,r){if(n instanceof M)return n;n.constructor&&n.constructor.name!==`Object`&&(n={data:n});let i=null;n.compressed&&(i={minFilter:`linear`,mipmapFilter:n.data.length>1?`nearest`:`linear`});let{width:a,height:o}=n.data,s=t.createTexture({...n,sampler:{...f_,...i,...r},mipLevels:t.getMipLevelCount(a,o)});return t.type===`webgl`?s.generateMipmapsWebGL():t.type===`webgpu`&&t.generateMipmapsWebGPU(s),p_[s.id]=e,s}function h_(e,t){!t||!(t instanceof M)||p_[t.id]===e&&(t.delete(),delete p_[t.id])}var g_={boolean:{validate(e,t){return!0},equal(e,t,n){return!!e==!!t}},number:{validate(e,t){return Number.isFinite(e)&&(!(`max`in t)||e<=t.max)&&(!(`min`in t)||e>=t.min)}},color:{validate(e,t){return t.optional&&!e||b_(e)&&(e.length===3||e.length===4)},equal(e,t,n){return Ip(e,t,1)}},accessor:{validate(e,t){let n=x_(e);return n===`function`||n===x_(t.value)},equal(e,t,n){return typeof t==`function`||Ip(e,t,1)}},array:{validate(e,t){return t.optional&&!e||b_(e)},equal(e,t,n){let{compare:r}=n;return r?Ip(e,t,Number.isInteger(r)?r:+!!r):e===t}},object:{equal(e,t,n){if(n.ignore)return!0;let{compare:r}=n;return r?Ip(e,t,Number.isInteger(r)?r:+!!r):e===t}},function:{validate(e,t){return t.optional&&!e||typeof e==`function`},equal(e,t,n){return!n.compare&&n.ignore!==!1||e===t}},data:{transform:(e,t,n)=>{if(!e)return e;let{dataTransform:r}=n.props;return r?r(e):typeof e.shape==`string`&&e.shape.endsWith(`-table`)&&Array.isArray(e.data)?e.data:e}},image:{transform:(e,t,n)=>{let r=n.context;return!r||!r.device?null:m_(n.id,r.device,e,{...t.parameters,...n.props.textureParameters})},release:(e,t,n)=>{h_(n.id,e)}}};function __(e){let t={},n={},r={};for(let[i,a]of Object.entries(e)){let e=a?.deprecatedFor;if(e)r[i]=Array.isArray(e)?e:[e];else{let e=v_(i,a);t[i]=e,n[i]=e.value}}return{propTypes:t,defaultProps:n,deprecatedProps:r}}function v_(e,t){switch(x_(t)){case`object`:return y_(e,t);case`array`:return y_(e,{type:`array`,value:t,compare:!1});case`boolean`:return y_(e,{type:`boolean`,value:t});case`number`:return y_(e,{type:`number`,value:t});case`function`:return y_(e,{type:`function`,value:t,compare:!0});default:return{name:e,type:`unknown`,value:t}}}function y_(e,t){return`type`in t?{name:e,...g_[t.type],...t}:`value`in t?{name:e,type:x_(t.value),...t}:{name:e,type:`object`,value:t}}function b_(e){return Array.isArray(e)||ArrayBuffer.isView(e)}function x_(e){return b_(e)?`array`:e===null?`null`:typeof e}function S_(e,t){let n;for(let e=t.length-1;e>=0;e--){let r=t[e];`extensions`in r&&(n=r.extensions)}let r=w_(e.constructor,n),i=Object.create(r);i[Sp]=e,i[Ep]={},i[Dp]={};for(let e=0;e<t.length;++e){let n=t[e];for(let e in n)i[e]=n[e]}return Object.freeze(i),i}var C_=`_mergedDefaultProps`;function w_(e,t){if(!(e instanceof P_.constructor))return{};let n=C_;if(t)for(let e of t){let t=e.constructor;t&&(n+=`:${t.extensionName||t.name}`)}return j_(e,n)||(e[n]=T_(e,t||[]))}function T_(e,t){if(!e.prototype)return null;let n=w_(Object.getPrototypeOf(e)),r=__(j_(e,`defaultProps`)||{}),i=Object.assign(Object.create(null),n,r.defaultProps),a=Object.assign(Object.create(null),n?.[Cp],r.propTypes),o=Object.assign(Object.create(null),n?.[wp],r.deprecatedProps);for(let e of t){let t=w_(e.constructor);t&&(Object.assign(i,t),Object.assign(a,t[Cp]),Object.assign(o,t[wp]))}return E_(i,e),O_(i,a),D_(i,o),i[Cp]=a,i[wp]=o,t.length===0&&!A_(e,`_propTypes`)&&(e._propTypes=a),i}function E_(e,t){let n=M_(t);Object.defineProperties(e,{id:{writable:!0,value:n}})}function D_(e,t){for(let n in t)Object.defineProperty(e,n,{enumerable:!1,set(e){let r=`${this.id}: ${n}`;for(let r of t[n])A_(this,r)||(this[r]=e);z.deprecated(r,t[n].join(`/`))()}})}function O_(e,t){let n={},r={};for(let e in t){let i=t[e],{name:a,value:o}=i;i.async&&(n[a]=o,r[a]=k_(a))}e[Tp]=n,e[Ep]={},Object.defineProperties(e,r)}function k_(e){return{enumerable:!0,set(t){typeof t==`string`||t instanceof Promise||hh(t)?this[Ep][e]=t:this[Dp][e]=t},get(){if(this[Dp]){if(e in this[Dp])return this[Dp][e]||this[Tp][e];if(e in this[Ep]){let t=this[Sp]&&this[Sp].internalState;if(t&&t.hasAsyncProp(e))return t.getAsyncProp(e)||this[Tp][e]}}return this[Tp][e]}}}function A_(e,t){return Object.prototype.hasOwnProperty.call(e,t)}function j_(e,t){return A_(e,t)&&e[t]}function M_(e){let t=e.componentName;return t||z.warn(`${e.name}.componentName not specified`)(),t||e.name}var N_=0,P_=class{constructor(...e){this.props=S_(this,e),this.id=this.props.id,this.count=N_++}clone(e){let{props:t}=this,n={};for(let e in t[Tp])e in t[Dp]?n[e]=t[Dp][e]:e in t[Ep]&&(n[e]=t[Ep][e]);return new this.constructor({...t,...n,...e})}};P_.componentName=`Component`,P_.defaultProps={};var F_=Object.freeze({}),I_=class{constructor(e){this.component=e,this.asyncProps={},this.onAsyncPropUpdated=()=>{},this.oldProps=null,this.oldAsyncProps=null}finalize(){for(let e in this.asyncProps){let t=this.asyncProps[e];t&&t.type&&t.type.release&&t.type.release(t.resolvedValue,t.type,this.component)}this.asyncProps={},this.component=null,this.resetOldProps()}getOldProps(){return this.oldAsyncProps||this.oldProps||F_}resetOldProps(){this.oldAsyncProps=null,this.oldProps=this.component?this.component.props:null}hasAsyncProp(e){return e in this.asyncProps}getAsyncProp(e){let t=this.asyncProps[e];return t&&t.resolvedValue}isAsyncPropLoading(e){if(e){let t=this.asyncProps[e];return!!(t&&t.pendingLoadCount>0&&t.pendingLoadCount!==t.resolvedLoadCount)}for(let e in this.asyncProps)if(this.isAsyncPropLoading(e))return!0;return!1}reloadAsyncProp(e,t){this._watchPromise(e,Promise.resolve(t))}setAsyncProps(e){this.component=e[Sp]||this.component;let t=e[Dp]||{},n=e[Ep]||e,r=e[Tp]||{};for(let e in t){let n=t[e];this._createAsyncPropData(e,r[e]),this._updateAsyncProp(e,n),t[e]=this.getAsyncProp(e)}for(let e in n){let t=n[e];this._createAsyncPropData(e,r[e]),this._updateAsyncProp(e,t)}}_fetch(e,t){return null}_onResolve(e,t){}_onError(e,t){}_updateAsyncProp(e,t){if(this._didAsyncInputValueChange(e,t)){if(typeof t==`string`&&(t=this._fetch(e,t)),t instanceof Promise){this._watchPromise(e,t);return}if(hh(t)){this._resolveAsyncIterable(e,t);return}this._setPropValue(e,t)}}_freezeAsyncOldProps(){if(!this.oldAsyncProps&&this.oldProps){this.oldAsyncProps=Object.create(this.oldProps);for(let e in this.asyncProps)Object.defineProperty(this.oldAsyncProps,e,{enumerable:!0,value:this.oldProps[e]})}}_didAsyncInputValueChange(e,t){let n=this.asyncProps[e];return t===n.resolvedValue||t===n.lastValue?!1:(n.lastValue=t,!0)}_setPropValue(e,t){this._freezeAsyncOldProps();let n=this.asyncProps[e];n&&(t=this._postProcessValue(n,t),n.resolvedValue=t,n.pendingLoadCount++,n.resolvedLoadCount=n.pendingLoadCount)}_setAsyncPropValue(e,t,n){let r=this.asyncProps[e];r&&n>=r.resolvedLoadCount&&t!==void 0&&(this._freezeAsyncOldProps(),r.resolvedValue=t,r.resolvedLoadCount=n,this.onAsyncPropUpdated(e,t))}_watchPromise(e,t){let n=this.asyncProps[e];if(n){n.pendingLoadCount++;let r=n.pendingLoadCount;t.then(t=>{this.component&&(t=this._postProcessValue(n,t),this._setAsyncPropValue(e,t,r),this._onResolve(e,t))}).catch(t=>{this._onError(e,t)})}}async _resolveAsyncIterable(e,t){if(e!==`data`){this._setPropValue(e,t);return}let n=this.asyncProps[e];if(!n)return;n.pendingLoadCount++;let r=n.pendingLoadCount,i=[],a=0;for await(let n of t){if(!this.component)return;let{dataTransform:t}=this.component.props;i=t?t(n,i):i.concat(n),Object.defineProperty(i,"__diff",{enumerable:!1,value:[{startRow:a,endRow:i.length}]}),a=i.length,this._setAsyncPropValue(e,i,r)}this._onResolve(e,i)}_postProcessValue(e,t){let n=e.type;return n&&this.component&&(n.release&&n.release(e.resolvedValue,n,this.component),n.transform)?n.transform(t,n,this.component):t}_createAsyncPropData(e,t){if(!this.asyncProps[e]){let n=this.component&&this.component.props[Cp];this.asyncProps[e]={type:n&&n[e],lastValue:null,resolvedValue:t,pendingLoadCount:0,resolvedLoadCount:0}}}},L_=class extends I_{constructor({attributeManager:e,layer:t}){super(t),this.attributeManager=e,this.needsRedraw=!0,this.needsUpdate=!0,this.subLayers=null,this.usesPickingColorCache=!1,this.disabledPickingIndices=[]}get layer(){return this.component}_fetch(e,t){let n=this.layer,r=n?.props.fetch;return r?r(t,{propName:e,layer:n}):super._fetch(e,t)}_onResolve(e,t){let n=this.layer;if(n){let r=n.props.onDataLoad;e===`data`&&r&&r(t,{propName:e,layer:n})}}_onError(e,t){let n=this.layer;n&&n.raiseError(t,`loading ${e} of ${this.layer}`)}},R_=`layer.changeFlag`,z_=`layer.initialize`,B_=`layer.update`,V_=`layer.finalize`,H_=`layer.matched`,U_=2**24-1,W_=Object.freeze([]),G_=od(({oldViewport:e,viewport:t})=>e.equals(t)),K_=new Uint8ClampedArray;function q_(e){return e.rowIndexes||e.pickingColors||e.instancePickingColors}function J_(e){return e.rowIndexes}function Y_(e){return e.pickingColors||e.instancePickingColors}var X_={data:{type:`data`,value:W_,async:!0},dataComparator:{type:`function`,value:null,optional:!0},_dataDiff:{type:`function`,value:e=>e&&e.__diff,optional:!0},dataTransform:{type:`function`,value:null,optional:!0},onDataLoad:{type:`function`,value:null,optional:!0},onError:{type:`function`,value:null,optional:!0},fetch:{type:`function`,value:(e,{propName:t,layer:n,loaders:r,loadOptions:i,signal:a})=>{let{resourceManager:o}=n.context;i||=n.getLoadOptions(),r||=n.props.loaders,a&&(i={...i,core:{...i?.core,fetch:{...i?.core?.fetch,signal:a}}});let s=o.contains(e);return!s&&!i&&(o.add({resourceId:e,data:sc(e,r),persistent:!1}),s=!0),s?o.subscribe({resourceId:e,onChange:e=>n.internalState?.reloadAsyncProp(t,e),consumerId:n.id,requestId:t}):sc(e,r,i)}},updateTriggers:{},visible:!0,pickable:!1,opacity:{type:`number`,min:0,max:1,value:1},operation:`draw`,onHover:{type:`function`,value:null,optional:!0},onClick:{type:`function`,value:null,optional:!0},onDragStart:{type:`function`,value:null,optional:!0},onDrag:{type:`function`,value:null,optional:!0},onDragEnd:{type:`function`,value:null,optional:!0},coordinateSystem:`default`,coordinateOrigin:{type:`array`,value:[0,0,0],compare:!0},modelMatrix:{type:`array`,value:null,compare:!0,optional:!0},wrapLongitude:!1,positionFormat:`XYZ`,colorFormat:`RGBA`,parameters:{type:`object`,value:{},optional:!0,compare:2},loadOptions:{type:`object`,value:null,optional:!0,ignore:!0},transitions:null,extensions:[],loaders:{type:`array`,value:[],optional:!0,ignore:!0},getPolygonOffset:{type:`function`,value:({layerIndex:e})=>[0,-e*100]},highlightedObjectIndex:null,autoHighlight:!1,highlightColor:{type:`accessor`,value:[0,0,128,128]}},Z_=class extends P_{constructor(){super(...arguments),this.internalState=null,this.lifecycle=xp.NO_STATE,this.parent=null}static get componentName(){return Object.prototype.hasOwnProperty.call(this,`layerName`)?this.layerName:``}get root(){let e=this;for(;e.parent;)e=e.parent;return e}toString(){return`${this.constructor.layerName||this.constructor.name}({id: '${this.props.id}'})`}project(e){H(this.internalState);let t=this.internalState.viewport||this.context.viewport,[n,r,i]=Xd(hp(e,{viewport:t,modelMatrix:this.props.modelMatrix,coordinateOrigin:this.props.coordinateOrigin,coordinateSystem:this.props.coordinateSystem}),t.pixelProjectionMatrix);return e.length===2?[n,r]:[n,r,i]}unproject(e){return H(this.internalState),(this.internalState.viewport||this.context.viewport).unproject(e)}projectPosition(e,t){return H(this.internalState),gp(e,{viewport:this.internalState.viewport||this.context.viewport,modelMatrix:this.props.modelMatrix,coordinateOrigin:this.props.coordinateOrigin,coordinateSystem:this.props.coordinateSystem,...t})}get isComposite(){return!1}get isDrawable(){return!0}setState(e){this.setChangeFlags({stateChanged:!0}),Object.assign(this.state,e),this.setNeedsRedraw()}setNeedsRedraw(){this.internalState&&(this.internalState.needsRedraw=!0)}setNeedsUpdate(){this.internalState&&(this.context.layerManager.setNeedsUpdate(String(this)),this.internalState.needsUpdate=!0)}get isLoaded(){return this.internalState?!this.internalState.isAsyncPropLoading():!1}get wrapLongitude(){return this.props.wrapLongitude}isPickable(){return this.props.pickable&&this.props.visible}getModels(){let e=this.state;return e&&(e.models||e.model&&[e.model])||[]}setShaderModuleProps(...e){for(let t of this.getModels())t.shaderInputs.setProps(...e)}getAttributeManager(){return this.internalState&&this.internalState.attributeManager}getCurrentLayer(){return this.internalState&&this.internalState.layer}getLoadOptions(){return this.props.loadOptions}use64bitPositions(){let{coordinateSystem:e}=this.props;return e==="default"||e===`lnglat`||e===`cartesian`}onHover(e,t){return this.props.onHover&&this.props.onHover(e,t)||!1}onClick(e,t){return this.props.onClick&&this.props.onClick(e,t)||!1}nullPickingColor(){return[0,0,0]}encodePickingColor(e,t=[]){return t[0]=e+1&255,t[1]=e+1>>8&255,t[2]=e+1>>8>>8&255,t}decodePickingColor(e){H(e instanceof Uint8Array);let[t,n,r]=e;return t+n*256+r*65536-1}getNumInstances(){return Number.isFinite(this.props.numInstances)?this.props.numInstances:this.state&&this.state.numInstances!==void 0?this.state.numInstances:c_(this.props.data)}getStartIndices(){return this.props.startIndices?this.props.startIndices:this.state&&this.state.startIndices?this.state.startIndices:null}getBounds(){return this.getAttributeManager()?.getBounds([`positions`,`instancePositions`])}getShaders(e){e=d_(e,{disableWarnings:!0,modules:this.context.defaultShaderModules});for(let t of this.props.extensions)e=d_(e,t.getShaders.call(this,t));return e}shouldUpdateState(e){return e.changeFlags.propsOrDataChanged}updateState(e){let t=this.getAttributeManager(),{dataChanged:n}=e.changeFlags;if(n&&t)if(Array.isArray(n))for(let e of n)t.invalidateAll(e);else t.invalidateAll();if(t){let{props:n}=e,r=this.internalState.hasPickingBuffer,i=Number.isInteger(n.highlightedObjectIndex)||!!n.pickable||n.extensions.some(e=>e.getNeedsPickingBuffer.call(this,e));if(r!==i){this.internalState.hasPickingBuffer=i;let e=q_(t.attributes);e&&(i&&e.constant&&(e.constant=!1,t.invalidate(e.id)),!e.value&&!i&&(e.constant=!0,e.value=J_(t.attributes)?[gf]:[0,0,0]))}}}finalizeState(e){for(let e of this.getModels())e.destroy();let t=this.getAttributeManager();t&&t.finalize(),this.context&&this.context.resourceManager.unsubscribe({consumerId:this.id}),this.internalState&&(this.internalState.uniformTransitions.clear(),this.internalState.finalize())}draw(e){for(let t of this.getModels())t.draw(e.renderPass)}getPickingInfo({info:e,mode:t,sourceLayer:n}){let{index:r}=e;return r>=0&&Array.isArray(this.props.data)&&(e.object=this.props.data[r]),e}raiseError(e,t){t&&(e=Error(`${t}: ${e.message}`,{cause:e})),this.props.onError?.(e)||this.context?.onError?.(e,this)}getNeedsRedraw(e={clearRedrawFlags:!1}){return this._getNeedsRedraw(e)}needsUpdate(){return this.internalState?this.internalState.needsUpdate||this.hasUniformTransition()||this.shouldUpdateState(this._getUpdateParams()):!1}hasUniformTransition(){return this.internalState?.uniformTransitions.active||!1}activateViewport(e){if(!this.internalState)return;let t=this.internalState.viewport;this.internalState.viewport=e,(!t||!G_({oldViewport:t,viewport:e}))&&(this.setChangeFlags({viewportChanged:!0}),this.isComposite?this.needsUpdate()&&this.setNeedsUpdate():this._update())}invalidateAttribute(e=`all`){let t=this.getAttributeManager();t&&(e===`all`?t.invalidateAll():t.invalidate(e))}updateAttributes(e){let t=!1;for(let n in e)e[n].layoutChanged()&&(t=!0);for(let n of this.getModels())this._setModelAttributes(n,e,t)}_updateAttributes(){let e=this.getAttributeManager();if(!e)return;let t=this.props,n=this.getNumInstances(),r=this.getStartIndices();e.update({data:t.data,numInstances:n,startIndices:r,props:t,transitions:t.transitions,buffers:t.data.attributes,context:this});let i=e.getChangedAttributes({clearChangedFlags:!0});this.updateAttributes(i)}_updateAttributeTransition(){let e=this.getAttributeManager();e&&e.updateTransition()}_updateUniformTransition(){let{uniformTransitions:e}=this.internalState;if(e.active){let t=e.update(),n=Object.create(this.props);for(let e in t)Object.defineProperty(n,e,{value:t[e]});return n}return this.props}calculateInstancePickingColors(e,{numInstances:t}){if(e.constant)return;let n=Math.floor(K_.length/4);this.internalState.usesPickingColorCache=!0;let r=t>0&&K_[0]===0;if(n<t||r){t>U_&&z.warn(`Layer has too many data objects. Picking might not be able to distinguish all objects.`)(),K_=Jf.allocate(K_,t,{size:4,copy:!0,maxCount:Math.max(t,U_)});let e=Math.floor(K_.length/4),i=[0,0,0],a=r?0:n;for(let t=a;t<e;t++)this.encodePickingColor(t,i),K_[t*4+0]=i[0],K_[t*4+1]=i[1],K_[t*4+2]=i[2],K_[t*4+3]=0}e.value=K_.subarray(0,t*4)}_setModelAttributes(e,t,n=!1){if(!Object.keys(t).length)return;let r=this.getAttributeManager();if(r?.hasBufferGroups()){this._setGroupedModelAttributes(e,r,t);return}if(n){let n=this.getAttributeManager();e.setBufferLayout(n.getBufferLayouts(e)),t=n.getAttributes()}let i=e.userData?.excludeAttributes||{},a={},o={};for(let n in t){if(i[n])continue;let r=t[n].getValue();for(let i in r){let s=r[i];s instanceof C?t[n].settings.isIndexed?e.setIndexBuffer(s):a[i]=s:s&&(o[i]=s)}}e.setAttributes(a),e.setConstantAttributes(o)}_setGroupedModelAttributes(e,t,n){let r=e.userData?.excludeAttributes||{},i=t.getBufferGroupBindings(n,e,r);e.setBufferLayout(i.bufferLayouts);let a={...i.buffers},o={},s=t.getAttributes();for(let t in s){if(r[t]||i.groupedAttributeIds.has(t))continue;let n=s[t],c=n.getValue();for(let t in c){let r=c[t];r instanceof C?n.settings.isIndexed?e.setIndexBuffer(r):a[t]=r:r&&(o[t]=r)}}e.setAttributes(a),e.setConstantAttributes(o)}disablePickingIndex(e){let t=this.props.data;if(!(`attributes`in t)){this._disablePickingIndex(e);return}let n=this.getAttributeManager().attributes,r=J_(n),i=Y_(n),a=r&&t.attributes&&t.attributes[r.id];if(a&&a.value){let n=a.value;for(let i=0;i<t.length;i++)n[r.getVertexOffset(i)]===e&&this._disablePickingIndex(i);return}let o=i&&t.attributes&&t.attributes[i.id];if(o&&o.value){let n=o.value,r=this.encodePickingColor(e);for(let e=0;e<t.length;e++){let t=i.getVertexOffset(e);n[t]===r[0]&&n[t+1]===r[1]&&n[t+2]===r[2]&&this._disablePickingIndex(e)}}else this._disablePickingIndex(e)}_disablePickingIndex(e){let t=this.getAttributeManager().attributes,n=J_(t);if(n){let t=n.getVertexOffset(e),r=n.getVertexOffset(e+1),i=new Uint32Array(r-t);i.fill(gf),n.buffer.write(i,t*i.BYTES_PER_ELEMENT);return}let r=Y_(t);if(!r){this.internalState&&_f(this.internalState.disabledPickingIndices,e);return}let i=r.getVertexOffset(e),a=r.getVertexOffset(e+1);r.buffer.write(new Uint8Array(a-i),i)}restorePickingColors(){let e=this.getAttributeManager().attributes,t=q_(e);if(!t){this.internalState&&(this.internalState.disabledPickingIndices.length=0);return}let n=Y_(e);this.internalState.usesPickingColorCache&&n&&n.value.buffer!==K_.buffer&&(n.value=K_.subarray(0,n.value.length)),t.updateSubBuffer({startOffset:0})}_initialize(){H(!this.internalState),el(z_,this);let e=this._getAttributeManager();this.internalState=new L_({attributeManager:e,layer:this}),this._clearChangeFlags(),this.state={},Object.defineProperty(this.state,"attributeManager",{get:()=>(z.deprecated(`layer.state.attributeManager`,`layer.getAttributeManager()`)(),e)}),this.internalState.uniformTransitions=new Xg(this.context.timeline),this.internalState.onAsyncPropUpdated=this._onAsyncPropUpdated.bind(this),this.internalState.setAsyncProps(this.props),this.initializeState(this.context);for(let e of this.props.extensions)e.initializeState.call(this,this.context,e);this.setChangeFlags({dataChanged:`init`,propsChanged:`init`,viewportChanged:!0,extensionsChanged:!0}),this._update()}_transferState(e){el(H_,this,this===e);let{state:t,internalState:n}=e;this!==e&&(this.internalState=n,this.state=t,this.internalState.setAsyncProps(this.props),this._diffProps(this.props,this.internalState.getOldProps()))}_update(){let e=this.needsUpdate();if(el(B_,this,e),!e)return;this.context.stats.get(`Layer updates`).incrementCount();let t=this.props,n=this.context,r=this.internalState,i=n.viewport,a=this._updateUniformTransition();r.propsInTransition=a,n.viewport=r.viewport||i,this.props=a;try{let e=this._getUpdateParams(),t=this.getModels();if(n.device)this.updateState(e);else try{this.updateState(e)}catch{}for(let t of this.props.extensions)t.updateState.call(this,e,t);this.setNeedsRedraw(),this._updateAttributes();let r=this.getModels()[0]!==t[0];this._postUpdate(e,r)}finally{n.viewport=i,this.props=t,this._clearChangeFlags(),r.needsUpdate=!1,r.resetOldProps()}}_finalize(){el(V_,this),this.finalizeState(this.context);for(let e of this.props.extensions)e.finalizeState.call(this,this.context,e)}_drawLayer({renderPass:e,shaderModuleProps:t=null,uniforms:n={},parameters:r={}}){this._updateAttributeTransition();let i=this.props,a=this.context;this.props=this.internalState.propsInTransition||i;try{t&&this.setShaderModuleProps(t);let{getPolygonOffset:i}=this.props,o=i&&i(n)||[0,0];a.device instanceof re&&a.device.setParametersWebGL({polygonOffset:o});let s=a.device instanceof re?null:Q_(r);if($_(this.getModels(),e,r,s),a.device instanceof re)a.device.withParametersWebGL(r,()=>{let i={renderPass:e,shaderModuleProps:t,uniforms:n,parameters:r,context:a};for(let e of this.props.extensions)e.draw.call(this,i,e);this.draw(i)});else{s?.renderPassParameters&&e.setParameters(s.renderPassParameters);let i={renderPass:e,shaderModuleProps:t,uniforms:n,parameters:r,context:a};for(let e of this.props.extensions)e.draw.call(this,i,e);this.draw(i)}}finally{this.props=i}}getChangeFlags(){return this.internalState?.changeFlags}setChangeFlags(e){if(!this.internalState)return;let{changeFlags:t}=this.internalState;for(let n in e)if(e[n]){let r=!1;switch(n){case`dataChanged`:let i=e[n],a=t[n];i&&Array.isArray(a)&&(t.dataChanged=Array.isArray(i)?a.concat(i):i,r=!0);default:t[n]||(t[n]=e[n],r=!0)}r&&el(R_,this,n,e)}let n=!!(t.dataChanged||t.updateTriggersChanged||t.propsChanged||t.extensionsChanged);t.propsOrDataChanged=n,t.somethingChanged=n||t.viewportChanged||t.stateChanged}_clearChangeFlags(){this.internalState.changeFlags={dataChanged:!1,propsChanged:!1,updateTriggersChanged:!1,viewportChanged:!1,stateChanged:!1,extensionsChanged:!1,propsOrDataChanged:!1,somethingChanged:!1}}_diffProps(e,t){let n=Qg(e,t);if(n.updateTriggersChanged)for(let e in n.updateTriggersChanged)n.updateTriggersChanged[e]&&this.invalidateAttribute(e);if(n.transitionsChanged)for(let r in n.transitionsChanged)this.internalState.uniformTransitions.add(r,t[r],e[r],e.transitions?.[r]);return this.setChangeFlags(n)}validateProps(){Zg(this.props)}updateAutoHighlight(e){this.props.autoHighlight&&!Number.isInteger(this.props.highlightedObjectIndex)&&this._updateAutoHighlight(e)}_updateAutoHighlight(e){let t={highlightedObjectColor:e.picked?e.color:null},{highlightColor:n}=this.props;e.picked&&typeof n==`function`&&(t.highlightColor=n(e)),this.setShaderModuleProps({picking:t}),this.setNeedsRedraw()}_getAttributeManager(){let e=this.context;return new Ug(e.device,{id:this.props.id,stats:e.stats,timeline:e.timeline})}_postUpdate(e,t){let{props:n,oldProps:r}=e,i=this.state.model;i?.isInstanced&&i.setInstanceCount(this.getNumInstances());let{autoHighlight:a,highlightedObjectIndex:o,highlightColor:s}=n;if(t||r.autoHighlight!==a||r.highlightedObjectIndex!==o||r.highlightColor!==s){let e={};Array.isArray(s)&&(e.highlightColor=s),(t||r.autoHighlight!==a||o!==r.highlightedObjectIndex)&&(e.highlightedObjectColor=Number.isFinite(o)&&o>=0?this.encodePickingColor(o):null),this.setShaderModuleProps({picking:e})}}_getUpdateParams(){return{props:this.props,oldProps:this.internalState.getOldProps(),context:this.context,changeFlags:this.internalState.changeFlags}}_getNeedsRedraw(e){if(!this.internalState)return!1;let t=!1;t||=this.internalState.needsRedraw&&this.id;let n=this.getAttributeManager(),r=n?n.getNeedsRedraw(e):!1;if(t||=r,t)for(let e of this.props.extensions)e.onNeedsRedraw.call(this,e);return this.internalState.needsRedraw=this.internalState.needsRedraw&&!e.clearRedrawFlags,t}_onAsyncPropUpdated(){this._diffProps(this.props,this.internalState.getOldProps()),this.setNeedsUpdate()}};Z_.defaultProps=X_,Z_.layerName=`Layer`;function Q_(e){let{blendConstant:t,...n}=e;return t?{pipelineParameters:n,renderPassParameters:{blendConstant:t}}:{pipelineParameters:n}}function $_(e,t,n,r){for(let i of e)i.device.type===`webgpu`?(ev(i,t),i.setParameters({...i.parameters,...r?.pipelineParameters})):i.setParameters(n)}function ev(e,t){let n=t.props.framebuffer||(t.framebuffer??null);if(!n)return;let r=n.colorAttachments.map(e=>e?.texture?.format??null),i=n.depthStencilAttachment?.texture?.format,a=e;(!tv(a.props.colorAttachmentFormats,r)||a.props.depthStencilAttachmentFormat!==i)&&(a.props.colorAttachmentFormats=r,a.props.depthStencilAttachmentFormat=i,a._setPipelineNeedsUpdate(`attachment formats`))}function tv(e,t){if(e===t)return!0;if(!e||!t||e.length!==t.length)return!1;for(let n=0;n<e.length;n++)if(e[n]!==t[n])return!1;return!0}var nv=`compositeLayer.renderLayers`,rv=class extends Z_{get isComposite(){return!0}get isDrawable(){return!1}get isLoaded(){return super.isLoaded&&this.getSubLayers().every(e=>e.isLoaded)}getSubLayers(){return this.internalState&&this.internalState.subLayers||[]}initializeState(e){}setState(e){super.setState(e),this.setNeedsUpdate()}getPickingInfo({info:e}){let{object:t}=e;return t&&t.__source&&t.__source.parent&&t.__source.parent.id===this.id?(e.object=t.__source.object,e.index=t.__source.index,e):e}filterSubLayer(e){return!0}shouldRenderSubLayer(e,t){return t&&t.length}getSubLayerClass(e,t){let{_subLayerProps:n}=this.props;return n&&n[e]&&n[e].type||t}getSubLayerRow(e,t,n){return e.__source={parent:this,object:t,index:n},e}getSubLayerAccessor(e){if(typeof e==`function`){let t={index:-1,data:this.props.data,target:[]};return(n,r)=>n&&n.__source?(t.index=n.__source.index,e(n.__source.object,t)):e(n,r)}return e}getSubLayerProps(e={}){let{opacity:t,pickable:n,visible:r,parameters:i,getPolygonOffset:a,highlightedObjectIndex:o,autoHighlight:s,highlightColor:c,coordinateSystem:l,coordinateOrigin:u,wrapLongitude:d,positionFormat:f,modelMatrix:p,extensions:m,fetch:h,operation:g,_subLayerProps:_}=this.props,v={id:``,updateTriggers:{},opacity:t,pickable:n,visible:r,parameters:i,getPolygonOffset:a,highlightedObjectIndex:o,autoHighlight:s,highlightColor:c,coordinateSystem:l,coordinateOrigin:u,wrapLongitude:d,positionFormat:f,modelMatrix:p,extensions:m,fetch:h,operation:g},y=_&&e.id&&_[e.id],b=y&&y.updateTriggers,x=e.id||`sublayer`;if(y){let t=this.props[Cp],n=e.type?e.type._propTypes:{};for(let e in y){let r=n[e]||t[e];r&&r.type===`accessor`&&(y[e]=this.getSubLayerAccessor(y[e]))}}Object.assign(v,e,y),v.id=`${this.props.id}-${x}`,v.updateTriggers={all:this.props.updateTriggers?.all,...e.updateTriggers,...b};for(let e of m){let t=e.getSubLayerProps.call(this,e);t&&Object.assign(v,t,{updateTriggers:Object.assign(v.updateTriggers,t.updateTriggers)})}return v}_updateAutoHighlight(e){for(let t of this.getSubLayers())t.updateAutoHighlight(e)}_getAttributeManager(){return null}_postUpdate(e,t){let n=this.internalState.subLayers,r=!n||this.needsUpdate();r&&(n=Op(this.renderLayers(),Boolean),this.internalState.subLayers=n),el(nv,this,r,n);for(let e of n)e.parent=this}};rv.layerName=`CompositeLayer`;var iv=Math.PI/180,av=180/Math.PI,ov=1,sv=6370972,cv=.75,lv=1.15;function uv(e){let t=Xf(e+180,360)-180;return Math.abs(t)<ov}function dv(){let e=256/sv,t=Math.PI/180*256;return{unitsPerMeter:[e,e,e],unitsPerMeter2:[0,0,0],metersPerUnit:[1/e,1/e,1/e],unitsPerDegree:[t,t,e],unitsPerDegree2:[0,0,0],degreesPerUnit:[1/t,1/t,1/e]}}var fv=class extends up{constructor(e={}){let{longitude:t=0,bearing:n=0,pitch:r=0,zoom:i=0,nearZMultiplier:a=.5,farZMultiplier:o=1,resolution:s=10}=e,{latitude:c=0,height:l,altitude:u=1.5,fovy:d}=e;c=Math.max(Math.min(c,90),-90),l||=1,d?u=Yd(d):d=Jd(u);let f=2**(i-pv(Math.max(Math.min(c,Ld),-Ld))),p=r*iv,m=e.nearZ??a,h=e.farZ??(u+256*2*f/l/Math.max(Math.cos(p),.1))*o,g=new R().lookAt({eye:[0,-u,0],up:[0,0,1]}).rotateX(-p).rotateY(-n*iv).rotateX(c*iv).rotateZ(-t*iv).scale(f/l);super({...e,height:l,viewMatrix:g,longitude:t,latitude:c,zoom:i,distanceScales:dv(),fovy:d,focalDistance:u,near:m,far:h}),this.scale=f,this.latitude=c,this.longitude=t,this.bearing=n,this.pitch=r,this.fovy=d,this.resolution=s}get projectionMode(){return td.GLOBE}getDistanceScales(){return this.distanceScales}getBounds(e={}){let t={targetZ:e.z||0},n=this.unproject([0,this.height/2],t),r=this.unproject([this.width/2,0],t),i=this.unproject([this.width,this.height/2],t),a=this.unproject([this.width/2,this.height],t);return i[0]<this.longitude&&(i[0]+=360),n[0]>this.longitude&&(n[0]-=360),[Math.min(n[0],i[0],r[0],a[0]),Math.min(n[1],i[1],r[1],a[1]),Math.max(n[0],i[0],r[0],a[0]),Math.max(n[1],i[1],r[1],a[1])]}_getRayToGlobe(e,{topLeft:t=!0,targetZ:n}={}){let[r,i]=e,a=t?i:this.height-i,{pixelUnprojectionMatrix:o}=this,s=mv(o,[r,a,-1,1]),c=mv(o,[r,a,1,1]),l=((n||0)/sv+1)*256,u=Tt(Ct([],s,c)),d=Tt(s),f=Tt(c);return{rayStartPosition:s,rayEndPosition:c,radius:l,rayLengthSquared:u,rayStartDistanceSquared:d,distanceToCenterSquared:4*((4*d*f-(u-d-f)**2)/16)/u}}_getRayDistanceToGlobeCenterRatio(e,t){let{distanceToCenterSquared:n,radius:r}=this._getRayToGlobe(e,t);return Math.sqrt(Math.max(0,n))/r}getZoomAnchorStrength(e){let t=this._getRayDistanceToGlobeCenterRatio(e);if(t>=lv)return 0;let n=Math.max(0,Math.min(1,(t-cv)/(lv-cv)));return 1-n*n*(3-2*n)}unproject(e,{topLeft:t=!0,targetZ:n}={}){let[r,i,a]=e,o=t?i:this.height-i,{pixelUnprojectionMatrix:s}=this,c;if(Number.isFinite(a))c=mv(s,[r,o,a,1]);else{let{rayStartPosition:r,rayEndPosition:i,radius:a,rayLengthSquared:o,rayStartDistanceSquared:s,distanceToCenterSquared:l}=this._getRayToGlobe(e,{topLeft:t,targetZ:n});c=ht([],r,i,(Math.sqrt(s-l)-Math.sqrt(Math.max(0,a*a-l)))/Math.sqrt(o))}let[l,u,d]=this.unprojectPosition(c);return Number.isFinite(a)?[l,u,d]:Number.isFinite(n)?[l,u,n]:[l,u]}projectPosition(e){let[t,n,r=0]=e,i=t*iv,a=n*iv,o=Math.cos(a),s=(r/sv+1)*256;return[Math.sin(i)*o*s,-Math.cos(i)*o*s,Math.sin(a)*s]}unprojectPosition(e){let[t,n,r]=e,i=wt(e),a=Math.asin(r/i);return[Math.atan2(t,-n)*av,a*av,(i/256-1)*sv]}projectFlat(e){return e}unprojectFlat(e){return e}panByPosition(e,t,n){if(!n){let n=this.getZoomAnchorStrength(t);if(n===0)return{longitude:this.longitude,latitude:this.latitude};let r=this.unproject(t),i=Xf(e[0]-r[0]+180,360)-180,a=e[1]-r[1],o=Math.abs(r[1])>85.051129||Math.abs(i)>90;if(uv(this.bearing)&&o)return{longitude:this.longitude,latitude:this.latitude};if(uv(this.bearing)&&a!==0){let e=((a>0?Ld:-Ld)-this.latitude)/a;n=Math.min(n,Math.max(0,e))}return{longitude:this.longitude+i*n,latitude:Math.max(Math.min(this.latitude+a*n,90),-90)}}let[r,i,a]=e,o=.25/2**(this.zoom-pv(this.latitude)),s=r+o*(n[0]-t[0]),c=i-o*(n[1]-t[1]);c=Math.max(Math.min(c,90),-90);let l={longitude:s,latitude:c,zoom:a-pv(i)};return l.zoom+=pv(l.latitude),l}};fv.displayName=`GlobeViewport`;function pv(e,t){t&&(e=Math.max(Math.min(e,Ld),-Ld));let n=Math.PI*Math.cos(e*Math.PI/180);return Math.log2(n)}function mv(e,t){let n=bn([],t,e);return mn(n,n,1/n[3]),n}var hv=Math.PI/180,gv=180/Math.PI,_v=class e{static toPosition(e,t){let n=t*hv,r=e*hv,i=Math.cos(n);return[i*Math.cos(r),i*Math.sin(r),Math.sin(n)]}static toLngLat(e){return[Math.atan2(e[1],e[0])*gv,Math.asin(P(e[2],-1,1))*gv]}static tangentBasis(e,t){let n=t*hv,r=e*hv,i=Math.sin(n),a=Math.cos(n),o=Math.sin(r),s=Math.cos(r);return{N:[-i*s,-i*o,a],E:[-o,s,0]}}static upVector(t,n,r){let{N:i,E:a}=e.tangentBasis(t,n),o=r*hv,s=Math.cos(o),c=Math.sin(o);return[i[0]*s+a[0]*c,i[1]*s+a[1]*c,i[2]*s+a[2]*c]}static bearing(t,n,r){let{N:i,E:a}=e.tangentBasis(n,r);return Math.atan2(pt(t,a),pt(t,i))*gv}static cameraFrame(t,n,r){let i=e.toPosition(t,n),a=e.upVector(t,n,r),{N:o,E:s}=e.tangentBasis(t,n),c=r*hv,l=Math.cos(c),u=Math.sin(c);return{position:i,up:a,axisHorizontal:mt([],i,[s[0]*l-o[0]*u,s[1]*l-o[1]*u,s[2]*l-o[2]*u]),axisVertical:mt([],i,a),longitude:t,latitude:n,bearing:r}}static angularDistance(t,n){let r=e.toPosition(t.longitude,t.latitude),i=e.toPosition(n.longitude,n.latitude);return Math.acos(P(pt(r,i),-1,1))}static greatCircleAxis(t,n){return ft([],mt([],e.toPosition(t.longitude,t.latitude),e.toPosition(n.longitude,n.latitude)))}static rotate(e,t,n){return vt([],e,new tr().fromAxisRotation(t,n))}static rotateFrame(t,n,r,i){let a=e.rotate(t.position,t.axisHorizontal,n);a=e.rotate(a,t.axisVertical,r);let o=e.rotate(t.up,t.axisHorizontal,n);o=e.rotate(o,t.axisVertical,r);let[s,c]=e.toLngLat(a),l=i?0:e.bearing(o,s,c);return{...t,position:a,up:o,longitude:s,latitude:c,bearing:l}}static rotateFrameToMatch(t,n,r,i=1){let a=e.toPosition(...n),o=e.toPosition(...r),s=mt([],a,o),c=wt(s),l=P(pt(a,o),-1,1);if(c<1e-12){if(l>0)return t;s=mt([],a,t.up),wt(s)<1e-12&&(s=mt([],a,t.axisVertical))}ft(s,s);let u=Math.atan2(c,l)*P(i,0,1),d=e.rotate(t.position,s,u),f=e.rotate(t.up,s,u),[p,m]=e.toLngLat(d);return{...t,position:d,up:f,longitude:p,latitude:m,bearing:e.bearing(f,p,m)}}},vv=1/(1-Math.exp(-5)),yv=e=>(1-Math.exp(-5*e))*vv,bv=class extends am{constructor(e){let t=`axis`in e;super({compare:[`longitude`,`latitude`],extract:t?[`longitude`,`latitude`,`zoom`,`bearing`]:[`longitude`,`latitude`,`zoom`],required:[`longitude`,`latitude`]}),t?(this._mode=`rotation`,this._axis=e.axis,this._totalAngle=e.totalAngle):(this._mode=`linear`,this._targetLongitude=e.targetLongitude)}initializeProps(e,t){let n=super.initializeProps(e,t);return this._startZoom=e.zoom,this._mode===`rotation`?this._startFrame={..._v.cameraFrame(e.longitude,e.latitude,e.bearing||0),axisHorizontal:this._axis}:n.end.longitude=this._targetLongitude,n}interpolateProps(e,t,n){if(this._mode===`rotation`){let{longitude:e,latitude:t,bearing:r}=_v.rotateFrame(this._startFrame,this._totalAngle*n,0);return{bearing:r,longitude:e,latitude:t,zoom:this._startZoom+pv(t,!0)-pv(this._startFrame.latitude,!0)}}let r=e.longitude+(t.longitude-e.longitude)*n,i=e.latitude+(t.latitude-e.latitude)*n;return{longitude:r,latitude:i,zoom:this._startZoom+pv(i,!0)-pv(e.latitude,!0)}}},xv=Math.PI/180,Sv=180/Math.PI;function Cv(e,t=0){let n=Math.min(180,e)*xv;return 256*2*Math.sin(n/2)*2**t}function wv(e,t=0){let n=e/2**t;return Math.asin(Math.min(1,n/256/2))*2*Sv}var Tv=class extends Om{constructor(e){let{startPanPos:t,startPanCameraFrame:n,startPanAngularRate:r,...i}=e;i.normalize=!1,super(i);let a=this._state;t!==void 0&&(a.startPanPos=t),n!==void 0&&(a.startPanCameraFrame=n),r!==void 0&&(a.startPanAngularRate=r)}panStart({pos:e}){let{latitude:t,longitude:n,zoom:r,bearing:i=0}=this.getViewportProps(),a=_v.cameraFrame(n,t,i),o=.25/2**(r-pv(t,!0))*xv;return this._getUpdatedState({startPanPos:e,startPanCameraFrame:a,startPanAngularRate:o,startZoom:r})}pan({pos:e,startPos:t}){let n=this.getState(),r=n.startPanPos||t;if(!r)return this;let i=n.startPanCameraFrame,a=n.startPanAngularRate,o=n.startZoom??this.getViewportProps().zoom;if(!i||!a)return this;let s=r[0]-e[0],c=r[1]-e[1],l=s*a,u=-c*a,d=_v.rotateFrame(i,l,u),f=o+pv(d.latitude,!0)-pv(i.latitude,!0);return this._getUpdatedState({longitude:d.longitude,latitude:d.latitude,bearing:d.bearing,zoom:f})}panEnd(){return this._getUpdatedState({startPanPos:null,startPanCameraFrame:null,startPanAngularRate:null,startZoom:null})}_panFromCenter(e){let{width:t,height:n}=this.getViewportProps(),r=[t/2,n/2];return this.panStart({pos:r}).pan({pos:[r[0]+e[0],r[1]+e[1]]}).panEnd()}applyConstraints(e){let t=e,n=t[_m];delete t[_m];let{latitude:r,maxBounds:i}=e;if(e.zoom=this._constrainZoom(e.zoom,e),n){let t=this.makeViewport(e),r=t.getZoomAnchorStrength(n.screenPosition);if(r>0){let i=t.unproject(n.screenPosition),a=_v.cameraFrame(e.longitude,e.latitude,e.bearing||0),o=_v.rotateFrameToMatch(a,[i[0],i[1]],[n.position[0],n.position[1]],r);e.longitude=o.longitude,e.latitude=o.latitude,e.bearing=o.bearing}}(e.longitude<-180||e.longitude>180)&&(e.longitude=Xf(e.longitude+180,360)-180),(e.bearing<-180||e.bearing>180)&&(e.bearing=Xf(e.bearing+180,360)-180),e.latitude=P(e.latitude,-90,90),e.pitch=P(e.pitch,e.minPitch,e.maxPitch);let a=i?bm(e.width,e.height,e.maxBoundsPadding):null;if(i&&a&&(a.width>=0&&(e.longitude=P(e.longitude,i[0][0],i[1][0])),a.height>=0&&(e.latitude=P(e.latitude,i[0][1],i[1][1]))),i&&a){let t=xm(this.makeViewport({...e,bearing:0,pitch:0}),[e.longitude,e.latitude],a),n=e.zoom-pv(r),o=i[1][0]-i[0][0],s=i[1][1]-i[0][1];if(a.height>=0&&s>0&&s<180){let r=Math.min(wv(a.height,n),s),o=a.height?r*t.bottom/a.height:wv(t.bottom,n),c=a.height?r*t.top/a.height:wv(t.top,n);e.latitude=P(e.latitude,i[0][1]+o,i[1][1]-c)}if(a.width>=0&&o>0&&o<360){let r=Math.min(wv(a.width/Math.cos(e.latitude*xv),n),o),s=a.width?r*t.left/a.width:wv(t.left/Math.cos(e.latitude*xv),n),c=a.width?r*t.right/a.width:wv(t.right/Math.cos(e.latitude*xv),n);e.longitude=P(e.longitude,i[0][0]+s,i[1][0]-c)}}return e.latitude=P(e.latitude,-90,90),e.latitude!==r&&(e.zoom+=pv(e.latitude,!0)-pv(r,!0)),e}_constrainZoom(e,t){t||=this.getViewportProps();let{maxZoom:n,maxBounds:r}=t,{minZoom:i}=t;if(r!==null&&t.width>0&&t.height>0){let e=bm(t.width,t.height,t.maxBoundsPadding),a=r[0][1],o=r[1][1],s=Math.sign(a)===Math.sign(o)?Math.min(Math.abs(a),Math.abs(o)):0,c=pv(0),l=Cv(r[1][0]-r[0][0])*Math.cos(s*xv),u=Cv(r[1][1]-r[0][1]);e.width>0&&l>0&&(i=Math.max(i,Math.log2(e.width/l)+c)),e.height>0&&u>0&&(i=Math.max(i,Math.log2(e.height/u)+c)),i>n&&(i=n)}let a=pv(t.latitude,!0)-pv(0,!0);return P(e,i+a,n+a)}},Ev=class extends gm{constructor(){super(...arguments),this.ControllerState=Tv,this.transition={transitionDuration:300,transitionInterpolator:new cm({transitionProps:{compare:[`longitude`,`latitude`,`zoom`,`bearing`,`pitch`],required:[`longitude`,`latitude`,`zoom`]}})},this.dragMode=`pan`,this._panHistory=[]}_onPanStart(e){return this._panHistory=[],super._onPanStart(e)}_onMultiPanStart(e){return this._panHistory=[],super._onMultiPanStart(e)}_onPanMove(e){if(!this.dragPan)return!1;let t=this.getCenter(e),n=this.controllerState.pan({pos:t});this.updateViewport(n,{transitionDuration:0},{isDragging:!0,isPanning:!0});let{longitude:r,latitude:i}=n.getViewportProps();return this._panHistory.push({longitude:r,latitude:i,timestamp:Date.now()}),this._panHistory.length>5&&this._panHistory.shift(),!0}_onPanMoveEnd(e){let{inertia:t}=this;if(this.dragPan&&t&&this._panHistory.length>=2){let e=this._panHistory[0],n=this._panHistory[this._panHistory.length-1],r=n.timestamp-e.timestamp;if(r>0){let i=this.controllerState.getViewportProps(),a=_v.angularDistance(e,n)/r;if(a>1e-6){let r=a*t/2,o=_v.greatCircleAxis(e,n),s=_v.cameraFrame(i.longitude,i.latitude,i.bearing||0),c=_v.rotateFrame({...s,axisHorizontal:o},r,0),l=c.longitude,u=P(c.latitude,-90,90),d=new bv({axis:o,totalAngle:r}),f=this.controllerState.panEnd();return this.updateViewport(f,{transitionInterpolator:d,transitionDuration:t,transitionEasing:yv,longitude:l,latitude:u},{isDragging:!1,isPanning:!0}),this._panHistory=[],!0}}}this._panHistory=[];let n=this.controllerState.panEnd();return this.updateViewport(n,null,{isDragging:!1,isPanning:!1}),!0}},Dv={cullMode:`back`},Ov=class extends Xp{constructor(e={}){super({...e,parameters:{...Dv,...e.parameters}})}getViewportType(e){return e.zoom>12?dp:fv}get ControllerType(){return Ev}};Ov.displayName=`GlobeView`;function kv(e){return e.getCenterElevation?e.getCenterElevation():e.getCameraTargetElevation?.()}function Av(e){let t=`mercator`;try{t=e.getProjection?.()?.type||`mercator`}catch{}if(t===`globe`)return`globe`;if(t!==`mercator`)throw Error(`Unsupported MapLibre projection: ${String(t)}`);return`mercator`}function jv(e,t){if(Mv(e))return e;if(Mv(t))return t;throw Error(`MapLibreOverlay interleaved rendering requires MapLibre GL JS 4.5.1 or later`)}function Mv(e){if(!e||typeof e!=`object`)return!1;let t=e;return Number.isFinite(t.nearZ)&&Number.isFinite(t.farZ)}var Nv=`deck-maplibre-layer-group-last`;function Pv(e){return e.props.beforeId?`deck-maplibre-layer-group-before:${e.props.beforeId}`:Nv}var Fv=`maplibre`,Iv=new WeakMap;function Lv(e,t){if(Iv.has(e))throw Error(`MapLibreOverlay supports one interleaved overlay per map`);let n=e.getCanvas().getContext(`webgl2`);if(!n)throw Error(`MapLibreOverlay interleaved rendering requires WebGL2.`);let{device:r,...i}=t,a=new rh({...i,gl:n});try{return Vv(e,a)}catch(t){throw Hv(e)===a?Uv(e):a.finalize(),t}}function Rv(e,t){return t?{depthWriteEnabled:!0,depthCompare:`less-equal`,depthBias:0,blend:!0,blendColorSrcFactor:`src-alpha`,blendColorDstFactor:`one-minus-src-alpha`,blendAlphaSrcFactor:`one`,blendAlphaDstFactor:`one-minus-src-alpha`,blendColorOperation:`add`,blendAlphaOperation:`add`}:{}}function zv(e){return Av(e)===`globe`?new Ov({id:Fv}):new Am({id:Fv})}function Bv(e){let{lng:t,lat:n}=e.getCenter(),r=e.getPadding(),i={longitude:(t+540)%360-180,latitude:n,zoom:e.getZoom(),bearing:e.getBearing(),pitch:e.getPitch(),padding:{top:r.top??0,bottom:r.bottom??0,left:r.left??0,right:r.right??0},repeat:e.getRenderWorldCopies()},a=kv(e);return typeof a==`number`&&Number.isFinite(a)&&(i.position=[0,0,a]),i}function Vv(e,t){if(Iv.get(e))throw Error(`MapLibreOverlay supports one interleaved overlay per map`);let n=t.props._customRender,r=t.props.onLoad,i={deck:t,watchingMove:!1,moveListener:()=>{t.isInitialized?Yv(t,e):Kv(e,i)},renderListener:()=>{t.isInitialized&&Jv(t,e)}};Iv.set(e,i);let a={...t.props,_customRender:()=>{e.triggerRepaint(),n?.(``)}};return a.views||=zv(e),Object.assign(a,{width:null,height:null,touchAction:`unset`,viewState:Bv(e)}),t.isInitialized?Gv(e,i):a.onLoad=()=>{r?.(),Iv.get(e)===i&&Gv(e,i)},t.setProps(a),e.on(`render`,i.renderListener),t}function Hv(e){return Iv.get(e)?.deck}function Uv(e){let t=Iv.get(e);t&&(Kv(e,t),e.off(`render`,t.renderListener),t.deck.finalize(),Iv.delete(e))}function Wv(e,t,n,r){if(!e.isInitialized)return;let{currentViewport:i}=e.userData,a=!1;i||(i=qv(e,t,r),e.userData.currentViewport=i,a=!0),i&&e._drawLayers(`maplibre-repaint`,{viewports:[i],layerFilter:t=>e.props.layerFilter&&!e.props.layerFilter(t)?!1:t.layer.props.beforeId===n.beforeId,clearStack:a,clearCanvas:!1})}function Gv(e,t){t.watchingMove||(t.watchingMove=!0,e.on(`move`,t.moveListener))}function Kv(e,t){t.watchingMove&&(t.watchingMove=!1,e.off(`move`,t.moveListener))}function qv(e,t,n){let r=Bv(t),i=e.getView(`maplibre`)||zv(t);if(n){i.props.nearZMultiplier=.2;let e=t.getCanvas().clientHeight;e>0&&Number.isFinite(n.nearZ)&&Number.isFinite(n.farZ)&&(r.nearZ=n.nearZ/e,r.farZ=n.farZ/e)}return i.makeViewport({width:e.width,height:e.height,viewState:r})}function Jv(e,t){let n=Op(e.props.layers,Boolean).some(e=>e&&!t.getLayer(Pv(e))),r=e.getViewports(),i=r.findIndex(e=>e.id===Fv),a=r.length>1||i<0;if(n||a){if(i>=0){r=r.slice();let n=qv(e,t);n?r[i]=n:r.splice(i,1)}e._drawLayers(`maplibre-repaint`,{viewports:r,layerFilter:n=>(!e.props.layerFilter||e.props.layerFilter(n))&&(n.viewport.id!==`maplibre`||!t.getLayer(Pv(n.layer))),clearCanvas:!1})}else{let t=e.device,n=t?.gl;e.props.onBeforeRender?.({device:t,gl:n}),e.props.onAfterRender?.({device:t,gl:n})}e.userData.currentViewport=null}function Yv(e,t){e.setProps({viewState:Bv(t)}),e.needsRedraw({clearRedrawFlags:!0})}var Xv=class{constructor(e){this.type=`custom`,this.map=null,H(e.id,`id is required`),this.id=e.id,this.renderingMode=e.renderingMode||`3d`,this.beforeId=e.beforeId}onAdd(e){this.map=e}onRemove(){this.map=null}render(e,t,n){if(!this.map)return;let r=Hv(this.map);r&&Wv(r,this.map,this,jv(t,n))}},Zv=new WeakMap;function Qv(e,t,n){if(!e||!e.isStyleLoaded())return;let r=Zv.get(e);r||(r=new Map,Zv.set(e,r));let i=Op(n,Boolean),a=new Set(i.map(Pv));if(t!==n){let n=Op(t,Boolean),i=new Set(n.map(Pv));for(let t of i)!a.has(t)&&r.has(t)&&(e.getLayer(t)&&e.removeLayer(t),r.delete(t))}for(let t of i){let n=Pv(t);if(r.has(n)){e.getLayer(n)||e.addLayer(r.get(n),t.props.beforeId);continue}if(e.getLayer(n))throw Error(`MapLibre style already contains a non-deck layer with id ${n}`);let i=new Xv({id:n,beforeId:t.props.beforeId});r.set(n,i),e.addLayer(i,t.props.beforeId)}for(let t of a){let n=r.get(t),i=e.getLayersOrder(),a=n.beforeId?i.indexOf(n.beforeId):i.length;a<0||i.indexOf(t)!==a-1&&e.moveLayer(t,n.beforeId)}}var $v=class{constructor(e){this._handleStyleChange=()=>{Qv(this._map,this._props.layers,this._props.layers),this._map&&Av(this._map)&&this._deck?.setProps({views:this._getViews(this._map)})},this._updateContainerSize=()=>{if(this._map&&this._container){let{clientWidth:e,clientHeight:t}=this._map.getContainer();Object.assign(this._container.style,{width:`${e}px`,height:`${t}px`})}},this._updateViewState=()=>{let e=this._deck,t=this._map;e&&t&&(e.setProps({views:this._getViews(t),viewState:Bv(t)}),e.isInitialized&&e.redraw())},this._handleMouseEvent=e=>{let t=this._deck;if(!t||!t.isInitialized)return;let n={type:e.type,offsetCenter:e.point||{x:0,y:0},srcEvent:e},r=this._lastMouseDownPoint;switch(!e.point&&r&&e.originalEvent instanceof MouseEvent&&(n.deltaX=e.originalEvent.clientX-r.clientX,n.deltaY=e.originalEvent.clientY-r.clientY,n.offsetCenter={x:r.x+n.deltaX,y:r.y+n.deltaY}),n.type){case`mousedown`:if(t._onPointerDown(n),!(e.originalEvent instanceof MouseEvent))return;this._lastMouseDownPoint={...n.offsetCenter,clientX:e.originalEvent.clientX,clientY:e.originalEvent.clientY};break;case`dragstart`:n.type=`panstart`,t._onEvent(n);break;case`drag`:n.type=`panmove`,t._onEvent(n);break;case`dragend`:n.type=`panend`,t._onEvent(n);break;case`click`:n.tapCount=1,t._onEvent(n);break;case`dblclick`:n.type=`click`,n.tapCount=2,t._onEvent(n);break;case`mousemove`:n.type=`pointermove`,t._onPointerMove(n);break;case`mouseout`:n.type=`pointerleave`,t._onPointerMove(n);break;default:return}};let{interleaved:t=!1}=e;this._interleaved=t,this._props=this._filterProps(e)}_filterProps(e){let{interleaved:t,useDevicePixels:n,device:r,...i}=e;return this._interleaved||(n!==void 0&&Object.assign(i,{useDevicePixels:n}),r!==void 0&&Object.assign(i,{device:r})),i}setProps(e){this._interleaved&&e.layers&&Qv(this._map,this._props.layers,e.layers),Object.assign(this._props,this._filterProps(e)),this._deck&&this._map&&this._deck.setProps({...this._props,views:this._getViews(this._map),parameters:{...Rv(this._map,this._interleaved),...this._props.parameters}})}onAdd(e){this._map=e;try{return this._interleaved?this._onAddInterleaved(e):this._onAddOverlaid(e)}catch(t){throw this._deck&&(this._interleaved?this._onRemoveInterleaved(e):this._onRemoveOverlaid(e)),this._deck=void 0,this._map=void 0,this._container=void 0,t}}onRemove(e){let t=this._map;t&&(this._interleaved?this._onRemoveInterleaved(t):this._onRemoveOverlaid(t)),this._deck=void 0,this._map=void 0,this._container=void 0}getDefaultPosition(){return`top-left`}pickObject(e){return H(this._deck),this._deck.pickObject(e)}pickMultipleObjects(e){return H(this._deck),this._deck.pickMultipleObjects(e)}pickObjects(e){return H(this._deck),this._deck.pickObjects(e)}finalize(){this._map?.removeControl(this)}getCanvas(){return this._map?this._interleaved?this._map.getCanvas():this._deck.getCanvas():null}_onAddOverlaid(e){let t=document.createElement(`div`);return Object.assign(t.style,{position:`absolute`,left:0,top:0,textAlign:`initial`,pointerEvents:`none`}),this._container=t,this._deck=new rh({...this._props,parent:t,deviceProps:{...this._props.deviceProps,createCanvasContext:{...typeof this._props.deviceProps?.createCanvasContext==`object`?this._props.deviceProps.createCanvasContext:void 0,pixelSizeSource:`css-dpr`}},parameters:{...Rv(e,!1),...this._props.parameters},views:this._getViews(e),viewState:Bv(e)}),e.on(`resize`,this._updateContainerSize),e.on(`render`,this._updateViewState),e.on(`mousedown`,this._handleMouseEvent),e.on(`dragstart`,this._handleMouseEvent),e.on(`drag`,this._handleMouseEvent),e.on(`dragend`,this._handleMouseEvent),e.on(`mousemove`,this._handleMouseEvent),e.on(`mouseout`,this._handleMouseEvent),e.on(`click`,this._handleMouseEvent),e.on(`dblclick`,this._handleMouseEvent),this._updateContainerSize(),t}_onAddInterleaved(e){return this._deck=Lv(e,{...this._props,views:this._getViews(e),parameters:{...Rv(e,!0),...this._props.parameters}}),e.on(`styledata`,this._handleStyleChange),Qv(e,[],this._props.layers),document.createElement(`div`)}_onRemoveOverlaid(e){e.off(`resize`,this._updateContainerSize),e.off(`render`,this._updateViewState),e.off(`mousedown`,this._handleMouseEvent),e.off(`dragstart`,this._handleMouseEvent),e.off(`drag`,this._handleMouseEvent),e.off(`dragend`,this._handleMouseEvent),e.off(`mousemove`,this._handleMouseEvent),e.off(`mouseout`,this._handleMouseEvent),e.off(`click`,this._handleMouseEvent),e.off(`dblclick`,this._handleMouseEvent),this._deck?.finalize()}_onRemoveInterleaved(e){e.off(`styledata`,this._handleStyleChange);try{Qv(e,this._props.layers,[])}finally{Uv(e)}}_getViews(e){if(!this._props.views)return zv(e);let t=Array.isArray(this._props.views)?this._props.views:[this._props.views];return t.some(e=>e.id===`maplibre`)?this._props.views:[zv(e),...t]}},ey=`layout(std140) uniform pointCloudUniforms {
  float radiusPixels;
  highp int sizeUnits;
} pointCloud;
`,ty={name:`pointCloud`,source:``,vs:ey,fs:ey,uniformTypes:{radiusPixels:`f32`,sizeUnits:`i32`}},ny=`#version 300 es
#define SHADER_NAME point-cloud-layer-vertex-shader
in vec3 positions;
in vec3 instanceNormals;
in vec4 instanceColors;
in vec3 instancePositions;
in vec3 instancePositions64Low;
out vec4 vColor;
out vec2 unitPosition;
void main(void) {
geometry.worldPosition = instancePositions;
geometry.normal = project_normal(instanceNormals);
unitPosition = positions.xy;
geometry.uv = unitPosition;
geometry.pickingColor = picking_getPickingColorFromInstanceID();
vec3 offset = vec3(positions.xy * project_size_to_pixel(pointCloud.radiusPixels, pointCloud.sizeUnits), 0.0);
DECKGL_FILTER_SIZE(offset, geometry);
#ifdef ANTIALIASING
float triangleRadiusPixels = length(offset.xy);
if (triangleRadiusPixels > 0.0) {
float coverageScale = 1.0 + 1.0 / project.devicePixelRatio / triangleRadiusPixels;
offset.xy *= coverageScale;
unitPosition *= coverageScale;
geometry.uv = unitPosition;
}
#endif
gl_Position = project_position_to_clipspace(instancePositions, instancePositions64Low, vec3(0.), geometry.position);
DECKGL_FILTER_GL_POSITION(gl_Position, geometry);
gl_Position.xy += project_pixel_size_to_clipspace(offset.xy);
vec3 lightColor = lighting_getLightColor(instanceColors.rgb, project.cameraPosition, geometry.position.xyz, geometry.normal);
vColor = vec4(lightColor, instanceColors.a * layer.opacity);
DECKGL_FILTER_COLOR(vColor, geometry);
}
`,ry=`#version 300 es
#define SHADER_NAME point-cloud-layer-fragment-shader
precision highp float;
in vec4 vColor;
in vec2 unitPosition;
out vec4 fragColor;
void main(void) {
geometry.uv = unitPosition.xy;
float distToCenter = length(unitPosition);
#ifdef ANTIALIASING
float edgePixels = (1.0 - distToCenter) / max(fwidth(distToCenter), 1e-6);
if (edgePixels <= -SMOOTH_EDGE_RADIUS) {
#else
if (distToCenter > 1.0) {
#endif
discard;
}
fragColor = vColor;
#ifdef ANTIALIASING
fragColor.a *= smoothedge(0.0, edgePixels);
#endif
DECKGL_FILTER_COLOR(fragColor, geometry);
}
`,iy=`struct PointCloudUniforms {
  radiusPixels: f32,
  sizeUnits: i32,
};

@group(0) @binding(0)
var<uniform> pointCloudUniforms: PointCloudUniforms;

struct Attributes {
  @builtin(instance_index) instanceIndex : u32,
  @builtin(vertex_index) vertexIndex : u32,
  @location(0) positions: vec3<f32>,
  @location(1) instancePositions: vec3<f32>,
  @location(2) instancePositions64Low: vec3<f32>,
  @location(3) instanceNormals: vec3<f32>,
  @location(4) instanceColors: vec4<f32>
};

struct Varyings {
  @builtin(position) position: vec4<f32>,
  @location(0) vColor: vec4<f32>,
  @location(1) unitPosition: vec2<f32>,
  @location(2) pickingColor: vec3<f32>,
};

@vertex
fn vertexMain(attributes: Attributes) -> Varyings {
  var varyings: Varyings;

  geometry.worldPosition = attributes.instancePositions;

  let centerResult = project_position_to_clipspace_and_commonspace(
    attributes.instancePositions,
    attributes.instancePositions64Low,
    vec3<f32>(0.0)
  );
  geometry.position = centerResult.commonPosition;
  geometry.normal = project_normal(attributes.instanceNormals);

  // Position on the enclosing triangle. Its edges are tangent to the unit circle.
  varyings.unitPosition = attributes.positions.xy;
  geometry.uv = varyings.unitPosition;
  geometry.pickingColor = picking_getPickingColorFromIndex(attributes.instanceIndex);

  // Find the center of the point and add the current vertex
#ifdef ANTIALIASING
  var offset = vec3<f32>(
#else
  let offset = vec3<f32>(
#endif
    attributes.positions.xy *
      project_unit_size_to_pixel(pointCloudUniforms.radiusPixels, pointCloudUniforms.sizeUnits),
    0.0
  );
  // DECKGL_FILTER_SIZE(offset, geometry);
#ifdef ANTIALIASING
  let triangleRadiusPixels = length(offset.xy);
  if (triangleRadiusPixels > 0.0) {
    // The triangle's inradius is half its vertex radius. Scaling its vertex radius by one device
    // pixel therefore adds half a device pixel around all three tangent points.
    let coverageScale = 1.0 + 1.0 / project.devicePixelRatio / triangleRadiusPixels;
    offset *= coverageScale;
    varyings.unitPosition *= coverageScale;
    geometry.uv = varyings.unitPosition;
  }
#endif

  varyings.position = centerResult.clipPosition;
  // DECKGL_FILTER_GL_POSITION(gl_Position, geometry);
  let clipPixels = project_pixel_size_to_clipspace(offset.xy);
  varyings.position.x += clipPixels.x;
  varyings.position.y += clipPixels.y;

  // Apply lighting
  let lightColor = lighting_getLightColor2(attributes.instanceColors.rgb, project.cameraPosition, geometry.position.xyz, geometry.normal);

  // Apply opacity to instance color, or return instance picking color
  varyings.vColor = vec4(lightColor, attributes.instanceColors.a * layer.opacity);
  // DECKGL_FILTER_COLOR(vColor, geometry);
  varyings.pickingColor = geometry.pickingColor;

  return varyings;
}

@fragment
fn fragmentMain(varyings: Varyings) -> @location(0) vec4<f32> {
  // var geometry: Geometry;
  // geometry.uv = unitPosition.xy;

  let distToCenter = length(varyings.unitPosition);
#ifdef ANTIALIASING
  let edgePixels = (1.0 - distToCenter) / max(fwidth(distToCenter), 1e-6);
  if (edgePixels <= -SMOOTH_EDGE_RADIUS) {
#else
  if (distToCenter > 1.0) {
#endif
    discard;
  }

  var fragColor: vec4<f32>;

  fragColor = varyings.vColor;

#ifdef ANTIALIASING
  fragColor.a *= smoothedge(0.0, edgePixels);
#endif

  if (picking.isActive > 0.5) {
    if (!picking_isColorValid(varyings.pickingColor)) {
      discard;
    }
    return vec4<f32>(varyings.pickingColor, 1.0);
  }

  if (picking.isHighlightActive > 0.5) {
    let highlightedObjectColor = picking_normalizeColor(picking.highlightedObjectColor);
    if (picking_isColorZero(abs(varyings.pickingColor - highlightedObjectColor))) {
      let highLightAlpha = picking.highlightColor.a;
      let blendedAlpha = highLightAlpha + fragColor.a * (1.0 - highLightAlpha);
      if (blendedAlpha > 0.0) {
        let highLightRatio = highLightAlpha / blendedAlpha;
        fragColor = vec4<f32>(
          mix(fragColor.rgb, picking.highlightColor.rgb, highLightRatio),
          blendedAlpha
        );
      } else {
        fragColor = vec4<f32>(fragColor.rgb, 0.0);
      }
    }
  }

  // Apply premultiplied alpha as required by transparent canvas
  fragColor = deckgl_premultiplied_alpha(fragColor);

  return fragColor;
}
`,ay=[0,0,0,255],oy=[0,0,1],sy={sizeUnits:`pixels`,pointSize:{type:`number`,min:0,value:10},antialiasing:!1,getPosition:{type:`accessor`,value:e=>e.position},getNormal:{type:`accessor`,value:oy},getColor:{type:`accessor`,value:ay},material:!0,radiusPixels:{deprecatedFor:`pointSize`}};function cy(e){let{header:t,attributes:n}=e;if(!(!t||!n)&&(e.length=t.vertexCount,n.POSITION&&(n.instancePositions=n.POSITION),n.NORMAL&&(n.instanceNormals=n.NORMAL),n.COLOR_0)){let{size:e,value:t}=n.COLOR_0;n.instanceColors={size:e,type:`unorm8`,value:t}}}var ly=class extends Z_{getShaders(){let{antialiasing:e}=this.props;return super.getShaders({vs:ny,fs:ry,source:iy,defines:e?{ANTIALIASING:1}:{},modules:[wd,cl,Kr,Cf,ty]})}initializeState(){this.getAttributeManager().addInstanced({instancePositions:{size:3,type:`float64`,fp64:this.use64bitPositions(),transition:!0,accessor:`getPosition`},instanceNormals:{size:3,transition:!0,accessor:`getNormal`,defaultValue:oy},instanceColors:{size:this.props.colorFormat.length,type:`unorm8`,transition:!0,accessor:`getColor`,defaultValue:ay}})}updateState(e){let{changeFlags:t,props:n,oldProps:r}=e;super.updateState(e),(t.extensionsChanged||n.antialiasing!==r.antialiasing)&&(this.state.model?.destroy(),this.state.model=this._getModel(),this.getAttributeManager().invalidateAll()),t.dataChanged&&cy(n.data)}draw({uniforms:e}){let{pointSize:t,sizeUnits:n}=this.props,r=this.state.model,i={sizeUnits:nd[n],radiusPixels:t};r.shaderInputs.setProps({pointCloud:i}),r.draw(this.context.renderPass)}_getModel(){let e=[];for(let t=0;t<3;t++){let n=t/3*Math.PI*2;e.push(Math.cos(n)*2,Math.sin(n)*2,0)}return new ue(this.context.device,{...this.getShaders(),id:this.props.id,bufferLayout:this.getAttributeManager().getBufferLayouts(),geometry:new de({topology:`triangle-list`,attributes:{positions:new Float32Array(e)}}),isInstanced:!0})}};ly.layerName=`PointCloudLayer`,ly.defaultProps=sy;var uy={OUTSIDE:-1,INTERSECTING:0,INSIDE:1};new I,new I;var dy=new I,fy=new I,py=class e{constructor(e=[0,0,0],t=0){this.radius=-0,this.center=new I,this.fromCenterRadius(e,t)}fromCenterRadius(e,t){return this.center.from(e),this.radius=t,this}fromCornerPoints(e,t){return t=dy.from(t),this.center=new I().from(e).add(t).scale(.5),this.radius=this.center.distance(t),this}equals(e){return this===e||!!e&&this.center.equals(e.center)&&this.radius===e.radius}clone(){return new e(this.center,this.radius)}union(e){let t=this.center,n=this.radius,r=e.center,i=e.radius,a=dy.copy(r).subtract(t),o=a.magnitude();if(n>=o+i)return this.clone();if(i>=o+n)return e.clone();let s=(n+o+i)*.5;return fy.copy(a).scale((-n+s)/o).add(t),this.center.copy(fy),this.radius=s,this}expand(e){let t=dy.from(e).subtract(this.center).magnitude();return t>this.radius&&(this.radius=t),this}transform(e){this.center.transform(e);let t=rn(dy,e);return this.radius=Math.max(t[0],Math.max(t[1],t[2]))*this.radius,this}distanceSquaredTo(e){let t=this.distanceTo(e);return t*t}distanceTo(e){let t=dy.from(e).subtract(this.center);return Math.max(0,t.len()-this.radius)}intersectPlane(e){let t=this.center,n=this.radius,r=e.normal.dot(t)+e.distance;return r<-n?uy.OUTSIDE:r<n?uy.INTERSECTING:uy.INSIDE}},my=new I,hy=new I,gy=new I,_y=new I,vy=new I,yy=new I,by=new I,xy={COLUMN0ROW0:0,COLUMN0ROW1:1,COLUMN0ROW2:2,COLUMN1ROW0:3,COLUMN1ROW1:4,COLUMN1ROW2:5,COLUMN2ROW0:6,COLUMN2ROW1:7,COLUMN2ROW2:8},Sy=class e{constructor(e=[0,0,0],t=[0,0,0,0,0,0,0,0,0]){this.center=new I().from(e),this.halfAxes=new L(t)}get halfSize(){let e=this.halfAxes.getColumn(0),t=this.halfAxes.getColumn(1),n=this.halfAxes.getColumn(2);return[new I(e).len(),new I(t).len(),new I(n).len()]}get quaternion(){let e=this.halfAxes.getColumn(0),t=this.halfAxes.getColumn(1),n=this.halfAxes.getColumn(2),r=new I(e).normalize(),i=new I(t).normalize(),a=new I(n).normalize();return new tr().fromMatrix3(new L([...r,...i,...a]))}fromCenterHalfSizeQuaternion(e,t,n){let r=new tr(n),i=new L().fromQuaternion(r);return i[0]*=t[0],i[1]*=t[0],i[2]*=t[0],i[3]*=t[1],i[4]*=t[1],i[5]*=t[1],i[6]*=t[2],i[7]*=t[2],i[8]*=t[2],this.center=new I().from(e),this.halfAxes=i,this}clone(){return new e(this.center,this.halfAxes)}equals(e){return this===e||!!e&&this.center.equals(e.center)&&this.halfAxes.equals(e.halfAxes)}getBoundingSphere(e=new py){let t=this.halfAxes,n=t.getColumn(0,gy),r=t.getColumn(1,_y),i=t.getColumn(2,vy),a=my.copy(n).add(r).add(i);return e.center.copy(this.center),e.radius=a.magnitude(),e}intersectPlane(e){let t=this.center,n=e.normal,r=this.halfAxes,i=n.x,a=n.y,o=n.z,s=Math.abs(i*r[xy.COLUMN0ROW0]+a*r[xy.COLUMN0ROW1]+o*r[xy.COLUMN0ROW2])+Math.abs(i*r[xy.COLUMN1ROW0]+a*r[xy.COLUMN1ROW1]+o*r[xy.COLUMN1ROW2])+Math.abs(i*r[xy.COLUMN2ROW0]+a*r[xy.COLUMN2ROW1]+o*r[xy.COLUMN2ROW2]),c=n.dot(t)+e.distance;return c<=-s?uy.OUTSIDE:c>=s?uy.INSIDE:uy.INTERSECTING}distanceTo(e){return Math.sqrt(this.distanceSquaredTo(e))}distanceSquaredTo(e){let t=hy.from(e).subtract(this.center),n=this.halfAxes,r=n.getColumn(0,gy),i=n.getColumn(1,_y),a=n.getColumn(2,vy),o=r.magnitude(),s=i.magnitude(),c=a.magnitude();r.normalize(),i.normalize(),a.normalize();let l=0,u;return u=Math.abs(t.dot(r))-o,u>0&&(l+=u*u),u=Math.abs(t.dot(i))-s,u>0&&(l+=u*u),u=Math.abs(t.dot(a))-c,u>0&&(l+=u*u),l}computePlaneDistances(e,t,n=[-0,-0]){let r=1/0,i=-1/0,a=this.center,o=this.halfAxes,s=o.getColumn(0,gy),c=o.getColumn(1,_y),l=o.getColumn(2,vy),u=yy.copy(s).add(c).add(l).add(a),d=by.copy(u).subtract(e),f=t.dot(d);return r=Math.min(f,r),i=Math.max(f,i),u.copy(a).add(s).add(c).subtract(l),d.copy(u).subtract(e),f=t.dot(d),r=Math.min(f,r),i=Math.max(f,i),u.copy(a).add(s).subtract(c).add(l),d.copy(u).subtract(e),f=t.dot(d),r=Math.min(f,r),i=Math.max(f,i),u.copy(a).add(s).subtract(c).subtract(l),d.copy(u).subtract(e),f=t.dot(d),r=Math.min(f,r),i=Math.max(f,i),a.copy(u).subtract(s).add(c).add(l),d.copy(u).subtract(e),f=t.dot(d),r=Math.min(f,r),i=Math.max(f,i),a.copy(u).subtract(s).add(c).subtract(l),d.copy(u).subtract(e),f=t.dot(d),r=Math.min(f,r),i=Math.max(f,i),a.copy(u).subtract(s).subtract(c).add(l),d.copy(u).subtract(e),f=t.dot(d),r=Math.min(f,r),i=Math.max(f,i),a.copy(u).subtract(s).subtract(c).subtract(l),d.copy(u).subtract(e),f=t.dot(d),r=Math.min(f,r),i=Math.max(f,i),n[0]=r,n[1]=i,n}transform(e){this.center.transformAsPoint(e);let t=this.halfAxes.getColumn(0,gy);t.transformAsPoint(e);let n=this.halfAxes.getColumn(1,_y);n.transformAsPoint(e);let r=this.halfAxes.getColumn(2,vy);return r.transformAsPoint(e),this.halfAxes=new L([...t,...n,...r]),this}getTransform(){throw Error(`not implemented`)}},Cy=new I,wy=new I,Ty=class e{constructor(e=[0,0,1],t=0){this.normal=new I,this.distance=-0,this.fromNormalDistance(e,t)}fromNormalDistance(e,t){return Ve(Number.isFinite(t)),this.normal.from(e).normalize(),this.distance=t,this}fromPointNormal(e,t){e=Cy.from(e),this.normal.from(t).normalize();let n=-this.normal.dot(e);return this.distance=n,this}fromCoefficients(e,t,n,r){return this.normal.set(e,t,n),Ve(Pe(this.normal.len(),1)),this.distance=r,this}clone(){return new e(this.normal,this.distance)}equals(e){return Pe(this.distance,e.distance)&&Pe(this.normal,e.normal)}getPointDistance(e){return this.normal.dot(e)+this.distance}transform(e){let t=wy.copy(this.normal).transformAsVector(e).normalize(),n=this.normal.scale(-this.distance).transform(e);return this.fromPointNormal(n,t)}projectPointOntoPlane(e,t=[0,0,0]){let n=Cy.from(e),r=this.getPointDistance(n),i=wy.copy(this.normal).scale(r);return n.subtract(i).to(t)}},Ey=[new I([1,0,0]),new I([0,1,0]),new I([0,0,1])],Dy=new I,Oy=new I,ky=class e{constructor(e=[]){this.planes=e}fromBoundingSphere(e){this.planes.length=2*Ey.length;let t=e.center,n=e.radius,r=0;for(let e of Ey){let i=this.planes[r],a=this.planes[r+1];i||=this.planes[r]=new Ty,a||=this.planes[r+1]=new Ty;let o=Dy.copy(e).scale(-n).add(t);i.fromPointNormal(o,e);let s=Dy.copy(e).scale(n).add(t),c=Oy.copy(e).negate();a.fromPointNormal(s,c),r+=2}return this}computeVisibility(e){let t=uy.INSIDE;for(let n of this.planes)switch(e.intersectPlane(n)){case uy.OUTSIDE:return uy.OUTSIDE;case uy.INTERSECTING:t=uy.INTERSECTING;break;default:}return t}computeVisibilityWithPlaneMask(t,n){if(Ve(Number.isFinite(n),`parentPlaneMask is required.`),n===e.MASK_OUTSIDE||n===e.MASK_INSIDE)return n;let r=e.MASK_INSIDE,i=this.planes;for(let a=0;a<this.planes.length;++a){let o=a<31?1<<a:0;if(a<31&&(n&o)===0)continue;let s=i[a],c=t.intersectPlane(s);if(c===uy.OUTSIDE)return e.MASK_OUTSIDE;c===uy.INTERSECTING&&(r|=o)}return r}};ky.MASK_OUTSIDE=4294967295,ky.MASK_INSIDE=0,ky.MASK_INDETERMINATE=2147483647,new I,new I,new I,new I,new I,new I,new I,new I,new I,new I,new I,new I,new I,new I,new I,new I,new I;var Ay=new L,jy=new L,My=new L,Ny=new L,Py=new L;function Fy(e,t={}){let n=ar,r=0,i=0,a=jy,o=My;a.identity(),o.copy(e);let s=n*Iy(o);for(;i<10&&zy(o)>s;)By(o,Ny),Py.copy(Ny).transpose(),o.multiplyRight(Ny),o.multiplyLeft(Py),a.multiplyRight(Ny),++r>2&&(++i,r=0);return t.unitary=a.toTarget(t.unitary),t.diagonal=o.toTarget(t.diagonal),t}function Iy(e){let t=0;for(let n=0;n<9;++n){let r=e[n];t+=r*r}return Math.sqrt(t)}var Ly=[1,0,0],Ry=[2,2,1];function zy(e){let t=0;for(let n=0;n<3;++n){let r=e[Ay.getElementIndex(Ry[n],Ly[n])];t+=2*r*r}return Math.sqrt(t)}function By(e,t){let n=ir,r=0,i=1;for(let t=0;t<3;++t){let n=Math.abs(e[Ay.getElementIndex(Ry[t],Ly[t])]);n>r&&(i=t,r=n)}let a=Ly[i],o=Ry[i],s=1,c=0;if(Math.abs(e[Ay.getElementIndex(o,a)])>n){let t=e[Ay.getElementIndex(o,o)],n=e[Ay.getElementIndex(a,a)],r=e[Ay.getElementIndex(o,a)],i=(t-n)/2/r,l;l=i<0?-1/(-i+Math.sqrt(1+i*i)):1/(i+Math.sqrt(1+i*i)),s=1/Math.sqrt(1+l*l),c=l*s}return L.IDENTITY.to(t),t[Ay.getElementIndex(a,a)]=t[Ay.getElementIndex(o,o)]=s,t[Ay.getElementIndex(o,a)]=c,t[Ay.getElementIndex(a,o)]=-c,t}var Vy=new I,Hy=new I,Uy=new I,Wy=new I,Gy=new I,Ky=new L,qy={diagonal:new L,unitary:new L};function Jy(e,t=new Sy){if(!e||e.length===0)return t.halfAxes=new L([0,0,0,0,0,0,0,0,0]),t.center=new I,t;let n=e.length,r=new I(0,0,0);for(let t of e)r.add(t);let i=1/n;r.multiplyByScalar(i);let a=0,o=0,s=0,c=0,l=0,u=0;for(let t of e){let e=Vy.copy(t).subtract(r);a+=e.x*e.x,o+=e.x*e.y,s+=e.x*e.z,c+=e.y*e.y,l+=e.y*e.z,u+=e.z*e.z}a*=i,o*=i,s*=i,c*=i,l*=i,u*=i;let d=Ky;d[0]=a,d[1]=o,d[2]=s,d[3]=o,d[4]=c,d[5]=l,d[6]=s,d[7]=l,d[8]=u;let{unitary:f}=Fy(d,qy),p=t.halfAxes.copy(f),m=p.getColumn(0,Uy),h=p.getColumn(1,Wy),g=p.getColumn(2,Gy),_=-Number.MAX_VALUE,v=-Number.MAX_VALUE,y=-Number.MAX_VALUE,b=Number.MAX_VALUE,x=Number.MAX_VALUE,S=Number.MAX_VALUE;for(let t of e)Vy.copy(t),_=Math.max(Vy.dot(m),_),v=Math.max(Vy.dot(h),v),y=Math.max(Vy.dot(g),y),b=Math.min(Vy.dot(m),b),x=Math.min(Vy.dot(h),x),S=Math.min(Vy.dot(g),S);m=m.multiplyByScalar(.5*(b+_)),h=h.multiplyByScalar(.5*(x+v)),g=g.multiplyByScalar(.5*(S+y)),t.center.copy(m).add(h).add(g);let C=Hy.set(_-b,v-x,y-S).multiplyByScalar(.5),w=new L([C[0],0,0,0,C[1],0,0,0,C[2]]);return t.halfAxes.multiplyRight(w),t}var Yy=Math.PI/180,Xy=new Float32Array(16),Zy=new Float32Array(12);function Qy(e,t,n){let r=t[0]*Yy,i=t[1]*Yy,a=t[2]*Yy,o=Math.sin(a),s=Math.sin(r),c=Math.sin(i),l=Math.cos(a),u=Math.cos(r),d=Math.cos(i),f=n[0],p=n[1],m=n[2];e[0]=f*d*u,e[1]=f*c*u,e[2]=f*-s,e[3]=p*(-c*l+d*s*o),e[4]=p*(d*l+c*s*o),e[5]=p*u*o,e[6]=m*(c*o+d*s*l),e[7]=m*(-d*o+c*s*l),e[8]=m*u*l}function $y(e){return e[0]=e[0],e[1]=e[1],e[2]=e[2],e[3]=e[4],e[4]=e[5],e[5]=e[6],e[6]=e[8],e[7]=e[9],e[8]=e[10],e[9]=e[12],e[10]=e[13],e[11]=e[14],e.subarray(0,12)}var eb={size:12,accessor:[`getOrientation`,`getScale`,`getTranslation`,`getTransformMatrix`],shaderAttributes:{instanceModelMatrixCol0:{size:3,elementOffset:0},instanceModelMatrixCol1:{size:3,elementOffset:3},instanceModelMatrixCol2:{size:3,elementOffset:6},instanceTranslation:{size:3,elementOffset:9}},update(e,{startRow:t,endRow:n}){let{data:r,getOrientation:i,getScale:a,getTranslation:o,getTransformMatrix:s}=this.props,c=Array.isArray(s),l=c&&s.length===16,u=Array.isArray(a),d=Array.isArray(i),f=Array.isArray(o),p=l||!c&&!!s(r[0]);p?e.constant=l:e.constant=d&&u&&f;let m=e.value;if(e.constant){let t;p?(Xy.set(s),t=$y(Xy)):(t=Zy,Qy(t,i,a),t.set(o,9)),e.value=new Float32Array(t)}else{let c=t*e.size,{iterable:h,objectInfo:g}=mh(r,t,n);for(let e of h){g.index++;let t;if(p)Xy.set(l?s:s(e,g)),t=$y(Xy);else{t=Zy;let n=d?i:i(e,g),r=u?a:a(e,g);Qy(t,n,r),t.set(f?o:o(e,g),9)}m[c++]=t[0],m[c++]=t[1],m[c++]=t[2],m[c++]=t[3],m[c++]=t[4],m[c++]=t[5],m[c++]=t[6],m[c++]=t[7],m[c++]=t[8],m[c++]=t[9],m[c++]=t[10],m[c++]=t[11]}}}};function tb(e,t){return t===`cartesian`||t===`meter-offsets`||t==="default"&&!e.isGeospatial}var nb=`struct SimpleMeshUniforms {
  sizeScale: f32,
  composeModelMatrix: f32,
  hasTexture: f32,
  flatShading: f32,
};

@group(0) @binding(auto) var<uniform> simpleMesh: SimpleMeshUniforms;
@group(0) @binding(auto) var simpleMeshTexture: texture_2d<f32>;
@group(0) @binding(auto) var simpleMeshTextureSampler: sampler;
`,rb=`layout(std140) uniform simpleMeshUniforms {
  float sizeScale;
  bool composeModelMatrix;
  bool hasTexture;
  bool flatShading;
} simpleMesh;
`,ib={name:`simpleMesh`,source:nb,vs:rb,fs:rb,uniformTypes:{sizeScale:`f32`,composeModelMatrix:`f32`,hasTexture:`f32`,flatShading:`f32`}},ab=`#version 300 es
#define SHADER_NAME simple-mesh-layer-vs
in vec3 positions;
in vec3 normals;
in vec3 colors;
in vec2 texCoords;
in vec3 instancePositions;
in vec3 instancePositions64Low;
in vec4 instanceColors;
in vec3 instanceModelMatrixCol0;
in vec3 instanceModelMatrixCol1;
in vec3 instanceModelMatrixCol2;
in vec3 instanceTranslation;
out vec2 vTexCoord;
out vec3 cameraPosition;
out vec3 normals_commonspace;
out vec4 position_commonspace;
out vec4 vColor;
void main(void) {
geometry.worldPosition = instancePositions;
geometry.uv = texCoords;
geometry.pickingColor = picking_getPickingColorFromInstanceID();
vTexCoord = texCoords;
cameraPosition = project.cameraPosition;
vColor = vec4(colors * instanceColors.rgb, instanceColors.a);
mat3 instanceModelMatrix = mat3(instanceModelMatrixCol0, instanceModelMatrixCol1, instanceModelMatrixCol2);
vec3 pos = (instanceModelMatrix * positions) * simpleMesh.sizeScale + instanceTranslation;
if (simpleMesh.composeModelMatrix) {
DECKGL_FILTER_SIZE(pos, geometry);
normals_commonspace = project_normal(instanceModelMatrix * normals);
geometry.worldPosition += pos;
gl_Position = project_position_to_clipspace(pos + instancePositions, instancePositions64Low, vec3(0.0), position_commonspace);
geometry.position = position_commonspace;
}
else {
pos = project_size(pos);
DECKGL_FILTER_SIZE(pos, geometry);
gl_Position = project_position_to_clipspace(instancePositions, instancePositions64Low, pos, position_commonspace);
geometry.position = position_commonspace;
normals_commonspace = project_normal(instanceModelMatrix * normals);
}
geometry.normal = normals_commonspace;
DECKGL_FILTER_GL_POSITION(gl_Position, geometry);
DECKGL_FILTER_COLOR(vColor, geometry);
}
`,ob=`#version 300 es
#define SHADER_NAME simple-mesh-layer-fs
precision highp float;
uniform sampler2D sampler;
in vec2 vTexCoord;
in vec3 cameraPosition;
in vec3 normals_commonspace;
in vec4 position_commonspace;
in vec4 vColor;
out vec4 fragColor;
void main(void) {
geometry.uv = vTexCoord;
vec3 normal;
if (simpleMesh.flatShading) {
normal = normalize(cross(dFdx(position_commonspace.xyz), dFdy(position_commonspace.xyz)));
} else {
normal = normals_commonspace;
}
vec4 color = simpleMesh.hasTexture ? texture(sampler, vTexCoord) : vColor;
DECKGL_FILTER_COLOR(color, geometry);
vec3 lightColor = lighting_getLightColor(color.rgb, cameraPosition, position_commonspace.xyz, normal);
fragColor = vec4(lightColor, color.a * layer.opacity);
}
`,sb=`struct Attributes {
  @builtin(instance_index) instanceIndex: u32,
  @location(0) positions: vec3<f32>,
  @location(1) normals: vec3<f32>,
  @location(2) colors: vec3<f32>,
  @location(3) texCoords: vec2<f32>,
  @location(4) instancePositions: vec3<f32>,
  @location(5) instancePositions64Low: vec3<f32>,
  @location(6) instanceColors: vec4<f32>,
  @location(7) instanceModelMatrixCol0: vec3<f32>,
  @location(8) instanceModelMatrixCol1: vec3<f32>,
  @location(9) instanceModelMatrixCol2: vec3<f32>,
  @location(10) instanceTranslation: vec3<f32>,
};

struct Varyings {
  @builtin(position) position: vec4<f32>,
  @location(0) color: vec4<f32>,
  @location(1) texCoords: vec2<f32>,
  @location(2) normal: vec3<f32>,
  @location(3) positionCommon: vec3<f32>,
  @location(4) pickingColor: vec3<f32>,
};

@vertex
fn vertexMain(attributes: Attributes) -> Varyings {
  var varyings: Varyings;

  geometry.worldPosition = attributes.instancePositions;
  geometry.uv = attributes.texCoords;
  geometry.pickingColor = picking_getPickingColorFromIndex(attributes.instanceIndex);

  let instanceModelMatrix = mat3x3<f32>(
    attributes.instanceModelMatrixCol0,
    attributes.instanceModelMatrixCol1,
    attributes.instanceModelMatrixCol2
  );
  let meshPosition =
    (instanceModelMatrix * attributes.positions) * simpleMesh.sizeScale +
    attributes.instanceTranslation;

  if (simpleMesh.composeModelMatrix > 0.5) {
    geometry.normal = project_normal(instanceModelMatrix * attributes.normals);
    geometry.worldPosition += meshPosition;
    let projected = project_position_to_clipspace_and_commonspace(
      attributes.instancePositions + meshPosition,
      attributes.instancePositions64Low,
      vec3<f32>(0.0)
    );
    geometry.position = projected.commonPosition;
    varyings.position = projected.clipPosition;
  } else {
    let projected = project_position_to_clipspace_and_commonspace(
      attributes.instancePositions,
      attributes.instancePositions64Low,
      project_size_vec3(meshPosition)
    );
    geometry.position = projected.commonPosition;
    geometry.normal = project_normal(instanceModelMatrix * attributes.normals);
    varyings.position = projected.clipPosition;
  }

  varyings.color = vec4<f32>(
    attributes.colors * attributes.instanceColors.rgb,
    attributes.instanceColors.a
  );
  varyings.texCoords = attributes.texCoords;
  varyings.normal = geometry.normal;
  varyings.positionCommon = geometry.position.xyz;
  varyings.pickingColor = geometry.pickingColor;
  return varyings;
}

@fragment
fn fragmentMain(varyings: Varyings) -> @location(0) vec4<f32> {
  geometry.uv = varyings.texCoords;

  if (picking.isActive > 0.5) {
    if (!picking_isColorValid(varyings.pickingColor)) {
      discard;
    }
    return vec4<f32>(varyings.pickingColor, 1.0);
  }

  var color = varyings.color;
  if (simpleMesh.hasTexture > 0.5) {
    color = textureSample(simpleMeshTexture, simpleMeshTextureSampler, varyings.texCoords);
  }

  var normal = varyings.normal;
  if (simpleMesh.flatShading > 0.5) {
    // WebGPU's screen-space Y axis reverses the derivative orientation used by GLSL flat shading.
    normal = normalize(cross(dpdy(varyings.positionCommon), dpdx(varyings.positionCommon)));
  }

  color = vec4<f32>(
    lighting_getLightColor2(color.rgb, project.cameraPosition, varyings.positionCommon, normal),
    color.a * layer.opacity
  );

  if (picking.isHighlightActive > 0.5) {
    let highlightedColor = picking_normalizeColor(picking.highlightedObjectColor);
    if (picking_isColorZero(abs(varyings.pickingColor - highlightedColor))) {
      let blendedAlpha = picking.highlightColor.a + color.a * (1.0 - picking.highlightColor.a);
      if (blendedAlpha > 0.0) {
        color = vec4<f32>(
          mix(color.rgb, picking.highlightColor.rgb, picking.highlightColor.a / blendedAlpha),
          blendedAlpha
        );
      }
    }
  }

  return deckgl_premultiplied_alpha(color);
}
`;function cb(e){let t=1/0,n=1/0,r=1/0,i=-1/0,a=-1/0,o=-1/0,s=e.POSITION?e.POSITION.value:[],c=s&&s.length;for(let e=0;e<c;e+=3){let c=s[e],l=s[e+1],u=s[e+2];t=c<t?c:t,n=l<n?l:n,r=u<r?u:r,i=c>i?c:i,a=l>a?l:a,o=u>o?u:o}return[[t,n,r],[i,a,o]]}function lb(e){let t=e.positions||e.POSITION;z.assert(t,`no "postions" or "POSITION" attribute in mesh`);let n=t.value.length/t.size,r=e.COLOR_0||e.colors;r||={size:3,value:new Float32Array(n*3).fill(1)};let i=e.NORMAL||e.normals;i||={size:3,value:new Float32Array(n*3).fill(0)};let a=e.TEXCOORD_0||e.texCoords;return a||={size:2,value:new Float32Array(n*2).fill(0)},{positions:t,colors:r,normals:i,texCoords:a}}function ub(e){return e instanceof de?(e.attributes=lb(e.attributes),e):e.attributes?new de({...e,topology:`triangle-list`,attributes:lb(e.attributes)}):new de({topology:`triangle-list`,attributes:lb(e)})}var db={mesh:{type:`object`,value:null,async:!0},texture:{type:`image`,value:null,async:!0},sizeScale:{type:`number`,value:1,min:0},_instanced:!0,wireframe:!1,material:!0,getPosition:{type:`accessor`,value:e=>e.position},getColor:{type:`accessor`,value:[0,0,0,255]},getOrientation:{type:`accessor`,value:[0,0,0]},getScale:{type:`accessor`,value:[1,1,1]},getTranslation:{type:`accessor`,value:[0,0,0]},getTransformMatrix:{type:`accessor`,value:[]},textureParameters:{type:`object`,ignore:!0,value:null}},fb=class extends Z_{getShaders(){return super.getShaders({vs:ab,fs:ob,source:sb,modules:[wd,cl,qr,Cf,ib]})}getBounds(){if(this.props._instanced)return super.getBounds();let e=this.state.positionBounds;if(e)return e;let{mesh:t}=this.props;if(!t)return null;if(e=t.header?.boundingBox,!e){let{attributes:n}=ub(t);n.POSITION=n.POSITION||n.positions,e=cb(n)}return this.state.positionBounds=e,e}initializeState(){this.getAttributeManager().addInstanced({instancePositions:{transition:!0,type:`float64`,fp64:this.use64bitPositions(),size:3,accessor:`getPosition`},instanceColors:{type:`unorm8`,transition:!0,size:this.props.colorFormat.length,accessor:`getColor`,defaultValue:[0,0,0,255]},instanceModelMatrix:eb}),this.setState({emptyTexture:this.context.device.createTexture({data:new Uint8Array(4),width:1,height:1})})}updateState(e){super.updateState(e);let{props:t,oldProps:n,changeFlags:r}=e;if(t.mesh!==n.mesh||r.extensionsChanged){if(this.state.positionBounds=null,this.state.model?.destroy(),t.mesh){this.state.model=this.getModel(t.mesh);let e=t.mesh.attributes||t.mesh;this.setState({hasNormals:!!(e.NORMAL||e.normals)})}this.getAttributeManager().invalidateAll()}t.texture!==n.texture&&t.texture instanceof M&&this.setTexture(t.texture),this.state.model&&this.state.model.setTopology(this.props.wireframe?`line-strip`:`triangle-list`)}finalizeState(e){super.finalizeState(e),this.state.emptyTexture.delete()}draw({uniforms:e}){let{model:t}=this.state;if(!t)return;let{viewport:n,renderPass:r}=this.context,{sizeScale:i,coordinateSystem:a,_instanced:o}=this.props,s={sizeScale:i,composeModelMatrix:!o||tb(n,a),flatShading:!this.state.hasNormals};t.shaderInputs.setProps({simpleMesh:s}),t.draw(r)}get isLoaded(){return!!(this.state?.model&&super.isLoaded)}getModel(e){let t=new ue(this.context.device,{...this.getShaders(),id:this.props.id,bufferLayout:this.getAttributeManager().getBufferLayouts(),geometry:ub(e),isInstanced:!0});return t.shaderInputs.setProps({simpleMesh:this.getTextureProps(this.props.texture)}),t}setTexture(e){let{model:t}=this.state;t&&t.shaderInputs.setProps({simpleMesh:this.getTextureProps(e)})}getTextureProps(e){let t=e||this.state.emptyTexture;return{...this.context.device.type===`webgpu`?{simpleMeshTexture:t}:{sampler:t},hasTexture:!!e}}};fb.defaultProps=db,fb.layerName=`SimpleMeshLayer`;var pb=`
struct VertexInputs {
  @location(0) positions: vec3f,
#ifdef HAS_NORMALS
  @location(1) normals: vec3f,
#endif
#ifdef HAS_TANGENTS
  @location(2) TANGENT: vec4f,
#endif
#ifdef HAS_UV
  @location(3) texCoords: vec2f,
#endif
#ifdef HAS_UV_1
  @location(4) texCoords1: vec2f,
#endif
#ifdef HAS_SKIN
  @location(5) JOINTS_0: vec4u,
  @location(6) WEIGHTS_0: vec4f,
#endif
#ifdef HAS_GLTF_INSTANCING
  @location(8) instanceModelMatrixCol0: vec4f,
  @location(9) instanceModelMatrixCol1: vec4f,
  @location(10) instanceModelMatrixCol2: vec4f,
  @location(11) instanceModelMatrixCol3: vec4f,
  @builtin(instance_index) instanceIndex: u32,
#endif
#ifdef HAS_GPU_CROWD_ANIMATION
  @location(12) instanceAnimationFrames: vec4f,
  @location(13) instanceAnimationBlend: vec4f,
#endif
#ifdef HAS_INSTANCED_MORPH
  @builtin(vertex_index) vertexIndex: u32,
#endif
};

struct FragmentInputs {
  @builtin(position) position: vec4f,
  @location(0) pbrPosition: vec3f,
  @location(1) pbrUV0: vec2f,
  @location(2) pbrUV1: vec2f,
  @location(3) pbrNormal: vec3f,
#ifdef HAS_TANGENTS
  @location(4) pbrTangent: vec4f,
#endif
};

#ifdef HAS_GLTF_INSTANCING
fn getGLTFInstanceNormalMatrix(matrix: mat3x3f) -> mat3x3f {
  let firstCofactor = cross(matrix[1], matrix[2]);
  let inverseDeterminant = 1.0 / dot(matrix[0], firstCofactor);
  return mat3x3f(
    firstCofactor,
    cross(matrix[2], matrix[0]),
    cross(matrix[0], matrix[1])
  ) * inverseDeterminant;
}
#endif

@vertex
fn vertexMain(inputs: VertexInputs) -> FragmentInputs {
  var outputs: FragmentInputs;
  var position = vec4f(inputs.positions, 1.0);
  var normal = vec3f(0.0, 0.0, 1.0);
  var tangent = vec4f(1.0, 0.0, 0.0, 1.0);
  var uv0 = vec2f(0.0, 0.0);
  var uv1 = vec2f(0.0, 0.0);

#ifdef HAS_NORMALS
  normal = inputs.normals;
#endif
#ifdef HAS_UV
  uv0 = inputs.texCoords;
#endif
#ifdef HAS_UV_1
  uv1 = inputs.texCoords1;
#endif
#ifdef HAS_TANGENTS
  tangent = inputs.TANGENT;
#endif

#ifdef HAS_INSTANCED_MORPH
  var animationFrames = vec4f(0.0);
  var animationBlend = vec4f(0.0);
#ifdef HAS_GPU_CROWD_ANIMATION
  animationFrames = inputs.instanceAnimationFrames;
  animationBlend = inputs.instanceAnimationBlend;
#endif
  position = vec4f(
    position.xyz + getGPUCrowdMorphDelta(
      inputs.instanceIndex,
      inputs.vertexIndex,
      0u,
      u32(CROWD_MORPH_VERTEX_COUNT),
      u32(CROWD_MORPH_TARGET_COUNT),
      u32(CROWD_ANIMATION_JOINT_COUNT),
      animationFrames,
      animationBlend,
      u32(CROWD_ANIMATION_FRAME_STRIDE)
    ),
    1.0
  );
#ifdef HAS_NORMALS
  normal = normalize(normal + getGPUCrowdMorphDelta(
    inputs.instanceIndex,
    inputs.vertexIndex,
    1u,
    u32(CROWD_MORPH_VERTEX_COUNT),
    u32(CROWD_MORPH_TARGET_COUNT),
    u32(CROWD_ANIMATION_JOINT_COUNT),
    animationFrames,
    animationBlend,
    u32(CROWD_ANIMATION_FRAME_STRIDE)
  ));
#endif
#ifdef HAS_TANGENTS
  tangent = vec4f(normalize(tangent.xyz + getGPUCrowdMorphDelta(
    inputs.instanceIndex,
    inputs.vertexIndex,
    2u,
    u32(CROWD_MORPH_VERTEX_COUNT),
    u32(CROWD_MORPH_TARGET_COUNT),
    u32(CROWD_ANIMATION_JOINT_COUNT),
    animationFrames,
    animationBlend,
    u32(CROWD_ANIMATION_FRAME_STRIDE)
  )), tangent.w);
#endif
#endif

#ifdef HAS_SKIN
#ifdef HAS_GPU_CROWD_ANIMATION
  let skinMatrix = getGPUAnimatedSkinMatrix(
    inputs.WEIGHTS_0,
    inputs.JOINTS_0,
    inputs.instanceAnimationFrames,
    inputs.instanceAnimationBlend,
    u32(CROWD_ANIMATION_FRAME_STRIDE)
  );
#else
#ifdef HAS_INSTANCED_SKIN
  let skinMatrix = getInstancedSkinMatrix(
    inputs.WEIGHTS_0,
    inputs.JOINTS_0,
    inputs.instanceIndex,
    u32(CROWD_JOINTS_PER_INSTANCE)
  );
#else
  let skinMatrix = getSkinMatrix(inputs.WEIGHTS_0, inputs.JOINTS_0);
#endif
#endif
  position = skinMatrix * position;
  normal = normalize((skinMatrix * vec4f(normal, 0.0)).xyz);
#ifdef HAS_TANGENTS
  tangent = vec4f(normalize((skinMatrix * vec4f(tangent.xyz, 0.0)).xyz), tangent.w);
#endif
#endif

#ifdef HAS_GLTF_INSTANCING
  var instanceMatrix = mat4x4f(
    inputs.instanceModelMatrixCol0,
    inputs.instanceModelMatrixCol1,
    inputs.instanceModelMatrixCol2,
    inputs.instanceModelMatrixCol3
  );
#ifdef HAS_GPU_CROWD_ANIMATION
  instanceMatrix *= sampleGPUAnimationMatrix(
    inputs.instanceAnimationFrames,
    inputs.instanceAnimationBlend,
    0u,
    u32(CROWD_ANIMATION_FRAME_STRIDE)
  );
#endif
  position = instanceMatrix * position;
  normal = normalize(getGLTFInstanceNormalMatrix(mat3x3f(
    instanceMatrix[0].xyz,
    instanceMatrix[1].xyz,
    instanceMatrix[2].xyz
  )) * normal);
#ifdef HAS_TANGENTS
  tangent = vec4f(normalize((instanceMatrix * vec4f(tangent.xyz, 0.0)).xyz), tangent.w);
#endif
#endif

  let worldPosition = pbrProjection.modelMatrix * position;

#ifdef HAS_NORMALS
  normal = normalize((pbrProjection.normalMatrix * vec4f(normal, 0.0)).xyz);
#endif
#ifdef HAS_TANGENTS
  let worldTangent = normalize((pbrProjection.modelMatrix * vec4f(tangent.xyz, 0.0)).xyz);
  outputs.pbrTangent = vec4f(worldTangent, tangent.w);
#endif

  outputs.position = pbrProjection.modelViewProjectionMatrix * position;
  outputs.pbrPosition = worldPosition.xyz / worldPosition.w;
  outputs.pbrUV0 = uv0;
  outputs.pbrUV1 = uv1;
  outputs.pbrNormal = normal;
  return outputs;
}

@fragment
fn fragmentMain(inputs: FragmentInputs) -> @location(0) vec4f {
  fragmentInputs.pbr_vPosition = inputs.pbrPosition;
  fragmentInputs.pbr_vUV0 = inputs.pbrUV0;
  fragmentInputs.pbr_vUV1 = inputs.pbrUV1;
  fragmentInputs.pbr_vNormal = inputs.pbrNormal;
#ifdef HAS_TANGENTS
  let tangent = normalize(inputs.pbrTangent.xyz);
  let bitangent = normalize(cross(inputs.pbrNormal, tangent)) * inputs.pbrTangent.w;
  fragmentInputs.pbr_vTBN = mat3x3f(tangent, bitangent, inputs.pbrNormal);
#endif
  return pbr_filterColor(vec4f(1.0));
}
`,mb=`#version 300 es

  // in vec4 POSITION;
  in vec4 positions;

  #ifdef HAS_NORMALS
    // in vec4 NORMAL;
    in vec4 normals;
  #endif

  #ifdef HAS_TANGENTS
    in vec4 TANGENT;
  #endif

  #ifdef HAS_UV
    // in vec2 TEXCOORD_0;
    in vec2 texCoords;
  #endif

  #ifdef HAS_UV_1
    in vec2 texCoords1;
  #endif

  #ifdef HAS_SKIN
    in uvec4 JOINTS_0;
    in vec4 WEIGHTS_0;
  #endif

  #ifdef HAS_GLTF_INSTANCING
    in vec4 instanceModelMatrixCol0;
    in vec4 instanceModelMatrixCol1;
    in vec4 instanceModelMatrixCol2;
    in vec4 instanceModelMatrixCol3;
  #endif

  #ifdef HAS_GPU_CROWD_ANIMATION
    in vec4 instanceAnimationFrames;
    in vec4 instanceAnimationBlend;
  #endif

  void main(void) {
    vec4 _NORMAL = vec4(0.);
    vec4 _TANGENT = vec4(0.);
    vec2 _TEXCOORD_0 = vec2(0.);
    vec2 _TEXCOORD_1 = vec2(0.);

    #ifdef HAS_NORMALS
      _NORMAL = normals;
    #endif

    #ifdef HAS_TANGENTS
      _TANGENT = TANGENT;
    #endif

    #ifdef HAS_UV
      _TEXCOORD_0 = texCoords;
    #endif

    #ifdef HAS_UV_1
      _TEXCOORD_1 = texCoords1;
    #endif

    vec4 pos = positions;

    #ifdef HAS_INSTANCED_MORPH
      vec4 animationFrames = vec4(0.0);
      vec4 animationBlend = vec4(0.0);
      #ifdef HAS_GPU_CROWD_ANIMATION
        animationFrames = instanceAnimationFrames;
        animationBlend = instanceAnimationBlend;
      #endif
      pos.xyz += getGPUCrowdMorphDelta(
        uint(gl_InstanceID),
        uint(gl_VertexID),
        0u,
        uint(CROWD_MORPH_TARGET_COUNT),
        uint(CROWD_ANIMATION_JOINT_COUNT),
        animationFrames,
        animationBlend
      );
      #ifdef HAS_NORMALS
        _NORMAL.xyz = normalize(_NORMAL.xyz + getGPUCrowdMorphDelta(
          uint(gl_InstanceID),
          uint(gl_VertexID),
          1u,
          uint(CROWD_MORPH_TARGET_COUNT),
          uint(CROWD_ANIMATION_JOINT_COUNT),
          animationFrames,
          animationBlend
        ));
      #endif
      #ifdef HAS_TANGENTS
        _TANGENT.xyz = normalize(_TANGENT.xyz + getGPUCrowdMorphDelta(
          uint(gl_InstanceID),
          uint(gl_VertexID),
          2u,
          uint(CROWD_MORPH_TARGET_COUNT),
          uint(CROWD_ANIMATION_JOINT_COUNT),
          animationFrames,
          animationBlend
        ));
      #endif
    #endif

    #ifdef HAS_SKIN
      #ifdef HAS_GPU_CROWD_ANIMATION
        mat4 skinMat = getGPUAnimatedSkinMatrix(
          WEIGHTS_0,
          JOINTS_0,
          instanceAnimationFrames,
          instanceAnimationBlend
        );
      #else
      #ifdef HAS_INSTANCED_SKIN
        mat4 skinMat = getInstancedSkinMatrix(
          WEIGHTS_0,
          JOINTS_0,
          uint(gl_InstanceID),
          uint(CROWD_JOINTS_PER_INSTANCE)
        );
      #else
      mat4 skinMat = getSkinMatrix(WEIGHTS_0, JOINTS_0);
      #endif
      #endif
      pos = skinMat * pos;
      _NORMAL = skinMat * _NORMAL;
      _TANGENT = vec4((skinMat * vec4(_TANGENT.xyz, 0.)).xyz, _TANGENT.w);
    #endif

    #ifdef HAS_GLTF_INSTANCING
      mat4 instanceMatrix = mat4(
        instanceModelMatrixCol0,
        instanceModelMatrixCol1,
        instanceModelMatrixCol2,
        instanceModelMatrixCol3
      );
      #ifdef HAS_GPU_CROWD_ANIMATION
        instanceMatrix *= sampleGPUAnimationMatrix(
          instanceAnimationFrames,
          instanceAnimationBlend,
          0
        );
      #endif
      pos = instanceMatrix * pos;
      _NORMAL = vec4(normalize(transpose(inverse(mat3(instanceMatrix))) * _NORMAL.xyz), 0.0);
      _TANGENT = vec4(normalize(mat3(instanceMatrix) * _TANGENT.xyz), _TANGENT.w);
    #endif

    pbr_setPositionNormalTangentUV(pos, _NORMAL, _TANGENT, _TEXCOORD_0, _TEXCOORD_1);
    gl_Position = pbrProjection.modelViewProjectionMatrix * pos;
  }
`,hb=`#version 300 es
  out vec4 fragmentColor;

  void main(void) {
    vec3 pos = pbr_vPosition;
    fragmentColor = pbr_filterColor(vec4(1.0));
  }
`;function gb(e,t){let n=t.materialFactory||new Di(e,{modules:[Qr]}),r={...t.parsedPPBRMaterial.uniforms};delete r.camera;let i=Object.fromEntries(Object.entries({...r,...t.parsedPPBRMaterial.bindings}).filter(([e,t])=>n.ownsBinding(e)&&yb(t))),a=n.createMaterial({id:t.id,bindings:i});return a.setProps({pbrMaterial:r}),a}function _b(e,t){let{id:n,geometry:r,parsedPPBRMaterial:i,vertexCount:a,modelOptions:o={},instanceMatrices:s,morphTargets:c=[]}=t,l=o.userData?.gltfAnimatedCrowd;if(l&&s)throw Error(`Nested glTF crowd instancing is unsupported`);w.info(4,`createGLTFModel defines: `,i.defines)();let u=[],d={depthWriteEnabled:!0,depthCompare:`less`,depthFormat:`depth24plus`,cullMode:`back`},f={},p=[],m=[],h=[],g=!!l?.gpuAnimation;if(s||l)for(let t=0;t<4;t++){let r=new Float32Array((l?.capacity||s?.length||0)*4);s?.forEach((e,n)=>{for(let i=0;i<4;i++)r[n*4+i]=e[t*4+i]});let i=`instanceModelMatrixCol${t}`,a=e.createBuffer({id:`${n||`gltf`}-${i}`,data:r,usage:C.VERTEX|C.COPY_DST});f[i]=a,p.push({name:i,format:`float32x4`,stepMode:`instance`}),u.push(a),m.push(a),h.push(r)}let _,v,y,b;if(l&&g){_=new Float32Array(l.capacity*4),y=new Float32Array(l.capacity*4);for(let[t,r]of[[`instanceAnimationFrames`,_],[`instanceAnimationBlend`,y]]){let i=e.createBuffer({id:`${n||`gltf`}-${t}`,data:r,usage:C.VERTEX|C.COPY_DST});f[t]=i,p.push({name:t,format:`float32x4`,stepMode:`instance`}),u.push(i),t===`instanceAnimationFrames`?v=i:b=i}}let x=!!i.defines.HAS_SKIN,S=!!(l&&x&&!g),T,E;l&&S&&(E=new Float32Array(l.capacity*l.jointsPerInstance*16),T=e.type===`webgpu`?e.createBuffer({id:`${n||`gltf`}-crowd-joint-matrices`,byteLength:E.byteLength,usage:C.STORAGE|C.COPY_DST}):e.createTexture({id:`${n||`gltf`}-crowd-joint-matrices`,format:`rgba32float`,width:l.jointsPerInstance*4,height:l.capacity,usage:M.SAMPLE|M.COPY_DST,sampler:{minFilter:`nearest`,magFilter:`nearest`,mipmapFilter:`nearest`}}),u.push(T));let D=l?c.length:0,O=Math.floor((r.attributes.POSITION?.value.length||0)/3),k,A,j;if(l&&D>0&&O>0){let t=new Float32Array(D*3*O*4);for(let[e,n]of c.entries())for(let[r,i]of[`POSITION`,`NORMAL`,`TANGENT`].entries()){let a=n[i];if(!a)continue;let o=i===`TANGENT`&&a.length===O*4?4:3;for(let n=0;n<O;n++){let i=((e*3+r)*O+n)*4,s=n*o;t[i]=a[s]||0,t[i+1]=a[s+1]||0,t[i+2]=a[s+2]||0}}if(k=vb(e,`${n||`gltf`}-crowd-morph-targets`,t,O,D*3),u.push(k),!g){let t=Math.ceil(D/4);A=new Float32Array(l.capacity*t*4),j=vb(e,`${n||`gltf`}-crowd-morph-weights`,A,t,l.capacity),u.push(j)}}let ee=x&&l?l.jointsPerInstance:0,te=4+ee*4+D,ne,re;l?.gpuAnimation&&(re=new Float32Array(l.gpuAnimation.frameCount*te*4),ne=vb(e,`${n||`gltf`}-crowd-animation-frames`,re,te,l.gpuAnimation.frameCount),u.push(ne));let ie=pb;for(let[e,t]of[[`CROWD_JOINTS_PER_INSTANCE`,l?.jointsPerInstance||0],[`CROWD_MORPH_VERTEX_COUNT`,O],[`CROWD_MORPH_TARGET_COUNT`,D],[`CROWD_ANIMATION_JOINT_COUNT`,ee],[`CROWD_ANIMATION_FRAME_STRIDE`,te]])ie=ie.replaceAll(`u32(${e})`,`u32(${t})`);let ae={id:n,source:ie,vs:mb,fs:hb,geometry:r,topology:r.topology,vertexCount:a,modules:[Qr,Tr,...l?[Dr]:[]],...o,...s||l?{attributes:{...o.attributes,...f},bufferLayout:[...o.bufferLayout||[],...p],instanceCount:s?.length||0,isInstanced:!0}:{},defines:{...i.defines,...o.defines,...s||l?{HAS_GLTF_INSTANCING:!0}:{},...S?{HAS_INSTANCED_SKIN:!0,CROWD_JOINTS_PER_INSTANCE:l.jointsPerInstance}:{},...g?{HAS_GPU_CROWD_ANIMATION:!0,CROWD_ANIMATION_FRAME_STRIDE:te}:{},...k?{HAS_INSTANCED_MORPH:!0,CROWD_MORPH_TARGET_COUNT:D}:{},...l?{CROWD_ANIMATION_JOINT_COUNT:ee}:{}},parameters:{...d,...i.parameters,...o.parameters}},oe=t.material||gb(e,{id:n?`${n}-material`:void 0,parsedPPBRMaterial:i});ae.material=oe;let se=new ue(e,ae),ce={...i.uniforms,...o.uniforms,...i.bindings,...o.bindings},le=bb(se.shaderInputs.getModules(),oe,ce);se.shaderInputs.setProps(le),T&&se.shaderInputs.setProps({skin:{jointMatrices:[],skinJointMatrices:T}}),(ne||k||j)&&se.shaderInputs.setProps({gpuAnimation:{...ne?{gpuAnimationFrames:ne}:{},...k?{gpuMorphTargets:k}:{},...j?{gpuMorphWeights:j}:{}}});let de=new ca({managedResources:u,model:se,bounds:t.bounds,instanceMatrices:s});return l&&(de.userData.gltfAnimatedCrowd={transformBuffers:m,transformColumns:h,skinJointMatrices:T,jointMatrices:E,jointsPerInstance:l.jointsPerInstance,morphTargetCount:D,morphTargetData:k,morphWeights:A,morphWeightData:j,animationFrames:ne,animationFrameValues:re,animationFrameStride:te,animationJointCount:ee,animationParameters:_,animationParameterBuffer:v,animationBlend:y,animationBlendBuffer:b}),de}function vb(e,t,n,r,i){if(e.type===`webgpu`)return e.createBuffer({id:t,data:n,usage:C.STORAGE|C.COPY_DST});let a=e.createTexture({id:t,format:`rgba32float`,width:r,height:i,usage:M.SAMPLE|M.COPY_DST,sampler:{minFilter:`nearest`,magFilter:`nearest`,mipmapFilter:`nearest`}});return a.writeData(n,{width:r,height:i}),a}function yb(e){return e instanceof C||e instanceof Yi||e instanceof se||e instanceof M||e instanceof ae}function bb(e,t,n){let r=new Map;for(let t of e){for(let e of Object.keys(t.uniformTypes||{}))r.set(e,t.name);for(let e of t.bindingLayout||[])r.set(e.name,t.name)}let i={};for(let[e,a]of Object.entries(n)){if(a===void 0)continue;let n=r.get(e);!n||t.ownsModule(n)||(i[n]||={},i[n][e]=a)}return i}function xb(e,t){let n=t.extensions?.EXT_mesh_gpu_instancing?.attributes;if(!n||typeof n!=`object`)return null;let r={},i;for(let[t,a]of Object.entries(n)){let n=typeof a==`number`?e.accessors[a]:a;if(!n||!ArrayBuffer.isView(n.value))throw Error(`Invalid glTF instance accessor for ${t}`);if(i!==void 0&&n.count!==i)throw Error(`glTF instance attributes must have matching accessor counts`);i=n.count,r[t]={value:n.value,size:n.components||Cb(n.type),count:n.count,normalized:!!n.normalized}}let a=[];for(let e=0;e<(i||0);e++){let t=Sb(r.TRANSLATION,e,[0,0,0]),n=Sb(r.ROTATION,e,[0,0,0,1]),i=Sb(r.SCALE,e,[1,1,1]),o=Math.hypot(...n);if(o>0)for(let e=0;e<n.length;e++)n[e]/=o;a.push(new R().translate(t).multiplyRight(new R().fromQuaternion(n)).scale(i))}return{matrices:a,attributes:r}}function Sb(e,t,n){if(!e)return[...n];let r=e.value;return n.map((n,i)=>{let a=r[t*e.size+i];return a===void 0?n:e.normalized?e.value instanceof Int8Array?Math.max(a/127,-1):e.value instanceof Int16Array?Math.max(a/32767,-1):e.value instanceof Uint8Array?a/255:e.value instanceof Uint16Array?a/65535:a:a})}function Cb(e){switch(e){case`VEC2`:return 2;case`VEC3`:return 3;case`VEC4`:return 4;default:return 1}}function wb(e,t){e.userData.morphWeights=[...t];let n=e.userData.morphMeshes||[];for(let e of n)e.preorderTraversal(e=>{if(!(e instanceof ca))return;let n=e.userData.morphTargets;n&&(pi(e.model,n.geometry,n.targets,t),e.userData.morphWeights=[...t])})}var U;(function(e){e[e.POINTS=0]=`POINTS`,e[e.LINES=1]=`LINES`,e[e.LINE_LOOP=2]=`LINE_LOOP`,e[e.LINE_STRIP=3]=`LINE_STRIP`,e[e.TRIANGLES=4]=`TRIANGLES`,e[e.TRIANGLE_STRIP=5]=`TRIANGLE_STRIP`,e[e.TRIANGLE_FAN=6]=`TRIANGLE_FAN`,e[e.ONE=1]=`ONE`,e[e.SRC_ALPHA=770]=`SRC_ALPHA`,e[e.ONE_MINUS_SRC_ALPHA=771]=`ONE_MINUS_SRC_ALPHA`,e[e.FUNC_ADD=32774]=`FUNC_ADD`,e[e.LINEAR=9729]=`LINEAR`,e[e.NEAREST=9728]=`NEAREST`,e[e.NEAREST_MIPMAP_NEAREST=9984]=`NEAREST_MIPMAP_NEAREST`,e[e.LINEAR_MIPMAP_NEAREST=9985]=`LINEAR_MIPMAP_NEAREST`,e[e.NEAREST_MIPMAP_LINEAR=9986]=`NEAREST_MIPMAP_LINEAR`,e[e.LINEAR_MIPMAP_LINEAR=9987]=`LINEAR_MIPMAP_LINEAR`,e[e.TEXTURE_MAG_FILTER=10240]=`TEXTURE_MAG_FILTER`,e[e.TEXTURE_MIN_FILTER=10241]=`TEXTURE_MIN_FILTER`,e[e.TEXTURE_WRAP_S=10242]=`TEXTURE_WRAP_S`,e[e.TEXTURE_WRAP_T=10243]=`TEXTURE_WRAP_T`,e[e.REPEAT=10497]=`REPEAT`,e[e.CLAMP_TO_EDGE=33071]=`CLAMP_TO_EDGE`,e[e.MIRRORED_REPEAT=33648]=`MIRRORED_REPEAT`,e[e.UNPACK_FLIP_Y_WEBGL=37440]=`UNPACK_FLIP_Y_WEBGL`})(U||={});function Tb(e){switch(e){case U.POINTS:return`point-list`;case U.LINES:return`line-list`;case U.LINE_STRIP:return`line-strip`;case U.TRIANGLES:return`triangle-list`;case U.TRIANGLE_STRIP:return`triangle-strip`;default:throw Error(String(e))}}function Eb(e,t,n){if(e!==U.LINE_LOOP&&e!==U.TRIANGLE_FAN)return{topology:Tb(e)};let r=t?.length??n,i=e===U.LINE_LOOP?r>=2?r*2:0:r>=3?(r-2)*3:0,a=new(t instanceof Uint32Array||!t&&n>65536?Uint32Array:Uint16Array)(i),o=e=>t?.[e]??e;if(e===U.LINE_LOOP){for(let e=0;e<r;e++)a[e*2]=o(e),a[e*2+1]=o((e+1)%r);return{topology:`line-list`,indices:a}}for(let e=0;e<r-2;e++)a[e*3]=o(0),a[e*3+1]=o(e+1),a[e*3+2]=o(e+2);return{topology:`triangle-list`,indices:a}}var Db=[W(`baseColor`,`pbr_baseColorSampler`,`baseColorTexture`,[`pbrMetallicRoughness`,`baseColorTexture`]),W(`metallicRoughness`,`pbr_metallicRoughnessSampler`,`metallicRoughnessTexture`,[`pbrMetallicRoughness`,`metallicRoughnessTexture`]),W(`normal`,`pbr_normalSampler`,`normalTexture`,[`normalTexture`]),W(`occlusion`,`pbr_occlusionSampler`,`occlusionTexture`,[`occlusionTexture`]),W(`emissive`,`pbr_emissiveSampler`,`emissiveTexture`,[`emissiveTexture`]),W(`specularColor`,`pbr_specularColorSampler`,`KHR_materials_specular.specularColorTexture`,[`extensions`,`KHR_materials_specular`,`specularColorTexture`]),W(`specularIntensity`,`pbr_specularIntensitySampler`,`KHR_materials_specular.specularTexture`,[`extensions`,`KHR_materials_specular`,`specularTexture`]),W(`transmission`,`pbr_transmissionSampler`,`KHR_materials_transmission.transmissionTexture`,[`extensions`,`KHR_materials_transmission`,`transmissionTexture`]),W(`thickness`,`pbr_thicknessSampler`,`KHR_materials_volume.thicknessTexture`,[`extensions`,`KHR_materials_volume`,`thicknessTexture`]),W(`clearcoat`,`pbr_clearcoatSampler`,`KHR_materials_clearcoat.clearcoatTexture`,[`extensions`,`KHR_materials_clearcoat`,`clearcoatTexture`]),W(`clearcoatRoughness`,`pbr_clearcoatRoughnessSampler`,`KHR_materials_clearcoat.clearcoatRoughnessTexture`,[`extensions`,`KHR_materials_clearcoat`,`clearcoatRoughnessTexture`]),W(`clearcoatNormal`,`pbr_clearcoatNormalSampler`,`KHR_materials_clearcoat.clearcoatNormalTexture`,[`extensions`,`KHR_materials_clearcoat`,`clearcoatNormalTexture`]),W(`sheenColor`,`pbr_sheenColorSampler`,`KHR_materials_sheen.sheenColorTexture`,[`extensions`,`KHR_materials_sheen`,`sheenColorTexture`]),W(`sheenRoughness`,`pbr_sheenRoughnessSampler`,`KHR_materials_sheen.sheenRoughnessTexture`,[`extensions`,`KHR_materials_sheen`,`sheenRoughnessTexture`]),W(`iridescence`,`pbr_iridescenceSampler`,`KHR_materials_iridescence.iridescenceTexture`,[`extensions`,`KHR_materials_iridescence`,`iridescenceTexture`]),W(`iridescenceThickness`,`pbr_iridescenceThicknessSampler`,`KHR_materials_iridescence.iridescenceThicknessTexture`,[`extensions`,`KHR_materials_iridescence`,`iridescenceThicknessTexture`]),W(`anisotropy`,`pbr_anisotropySampler`,`KHR_materials_anisotropy.anisotropyTexture`,[`extensions`,`KHR_materials_anisotropy`,`anisotropyTexture`]),W(`bump`,`pbr_bumpSampler`,`EXT_materials_bump.bumpTexture`,[`extensions`,`EXT_materials_bump`,`bumpTexture`]),W(`diffuseTransmission`,`pbr_diffuseTransmissionSampler`,`KHR_materials_diffuse_transmission.diffuseTransmissionTexture`,[`extensions`,`KHR_materials_diffuse_transmission`,`diffuseTransmissionTexture`]),W(`diffuseTransmissionColor`,`pbr_diffuseTransmissionColorSampler`,`KHR_materials_diffuse_transmission.diffuseTransmissionColorTexture`,[`extensions`,`KHR_materials_diffuse_transmission`,`diffuseTransmissionColorTexture`]),W(`multiscatterColor`,`pbr_multiscatterColorSampler`,`KHR_materials_volume_scatter.multiscatterColorTexture`,[`extensions`,`KHR_materials_volume_scatter`,`multiscatterColorTexture`])],Ob=new Map(Db.map(e=>[e.slot,e]));function W(e,t,n,r){return{slot:e,binding:t,displayName:n,pathSegments:r,colorSpace:e===`baseColor`||e===`emissive`||e===`specularColor`||e===`sheenColor`||e===`diffuseTransmissionColor`||e===`multiscatterColor`?`srgb`:`linear`,uvSetUniform:`${e}UVSet`,uvTransformUniform:`${e}UVTransform`}}function kb(){return Db}function Ab(e){let t=Ob.get(e);if(!t)throw Error(`Unknown PBR texture transform slot ${e}`);return t}function jb(e){let t=e?.extensions?.KHR_texture_transform;return{offset:t?.offset?[t.offset[0],t.offset[1]]:[0,0],rotation:t?.rotation??0,scale:t?.scale?[t.scale[0],t.scale[1]]:[1,1]}}function Mb(e){return e?.extensions?.KHR_texture_transform?.texCoord??e?.texCoord??0}function Nb(e){return Db.find(t=>t.pathSegments.length===e.length&&t.pathSegments.every((t,n)=>e[n]===t))||null}function Pb(e){let t=new L().set(1,0,0,0,1,0,e.offset[0],e.offset[1],1),n=new L().set(Math.cos(e.rotation),Math.sin(e.rotation),0,-Math.sin(e.rotation),Math.cos(e.rotation),0,0,0,1),r=new L().set(e.scale[0],0,0,0,e.scale[1],0,0,0,1);return Array.from(t.multiplyRight(n).multiplyRight(r))}function Fb(e,t){let n=new L(Pb(e)),r=new L(Pb(t)),i=new L(n).invert();return Array.from(r.multiplyRight(i))}function Ib(e={}){let t=e.wrapS??e.parameters?.[U.TEXTURE_WRAP_S],n=e.wrapT??e.parameters?.[U.TEXTURE_WRAP_T],r=e.magFilter??e.parameters?.[U.TEXTURE_MAG_FILTER],i=e.minFilter??e.parameters?.[U.TEXTURE_MIN_FILTER],a=Lb(t),o=Lb(n),s=Rb(r);return{...a?{addressModeU:a}:{},...o?{addressModeV:o}:{},...s?{magFilter:s}:{},...zb(i)}}function Lb(e){switch(e){case U.CLAMP_TO_EDGE:return`clamp-to-edge`;case U.REPEAT:return`repeat`;case U.MIRRORED_REPEAT:return`mirror-repeat`;default:return}}function Rb(e){switch(e){case U.NEAREST:return`nearest`;case U.LINEAR:return`linear`;default:return}}function zb(e){switch(e){case U.NEAREST:return{minFilter:`nearest`};case U.LINEAR:return{minFilter:`linear`};case U.NEAREST_MIPMAP_NEAREST:return{minFilter:`nearest`,mipmapFilter:`nearest`};case U.LINEAR_MIPMAP_NEAREST:return{minFilter:`linear`,mipmapFilter:`nearest`};case U.NEAREST_MIPMAP_LINEAR:return{minFilter:`nearest`,mipmapFilter:`linear`};case U.LINEAR_MIPMAP_LINEAR:return{minFilter:`linear`,mipmapFilter:`linear`};default:return{}}}var Bb={NORMAL:[`NORMAL`,`normals`],TANGENT:[`TANGENT`],TEXCOORD_0:[`TEXCOORD_0`,`texCoords`],TEXCOORD_1:[`TEXCOORD_1`,`texCoords1`],JOINTS_0:[`JOINTS_0`],WEIGHTS_0:[`WEIGHTS_0`],COLOR_0:[`COLOR_0`,`colors`]};function Vb(e,t,n,r){let i={defines:{MANUAL_SRGB:!0},bindings:{},uniforms:{camera:[0,0,0],metallicRoughnessValues:[1,1]},parameters:{},glParameters:{},generatedTextures:[]};i.defines.USE_TEX_LOD=!0;let{imageBasedLightingEnvironment:a}=r;return a&&(i.bindings.pbr_diffuseEnvSampler=a.diffuseEnvSampler.texture,i.bindings.pbr_specularEnvSampler=a.specularEnvSampler.texture,i.bindings.pbr_brdfLUT=a.brdfLutTexture.texture,i.uniforms.IBLenabled=!0,i.uniforms.scaleIBLAmbient=[1,1]),r?.pbrDebug&&(i.defines.PBR_DEBUG=!0,i.uniforms.scaleDiffBaseMR=[0,0,0,0],i.uniforms.scaleFGDSpec=[0,0,0,0]),Wb(n,`NORMAL`)&&(i.defines.HAS_NORMALS=!0),Wb(n,`TANGENT`)&&r?.useTangents&&(i.defines.HAS_TANGENTS=!0),Wb(n,`TEXCOORD_0`)&&(i.defines.HAS_UV=!0),Wb(n,`TEXCOORD_1`)&&(i.defines.HAS_UV_1=!0),Wb(n,`JOINTS_0`)&&Wb(n,`WEIGHTS_0`)&&(i.defines.HAS_SKIN=!0),Wb(n,`COLOR_0`)&&(i.defines.HAS_COLORS=!0),r?.imageBasedLightingEnvironment&&(i.defines.USE_IBL=!0),r?.lights&&(i.defines.USE_LIGHTS=!0),t&&(r.validateAttributes!==!1&&Hb(t,n),Kb(e,t,i,n,r.gltf)),i}function Hb(e,t){let n=Ub(e,0);n.length>0&&!Wb(t,`TEXCOORD_0`)&&w.warn(`glTF material uses ${n.join(`, `)} but primitive is missing TEXCOORD_0; textured shading will sample the default UV coordinates`)();let r=Ub(e,1);if(r.length>0&&!Wb(t,`TEXCOORD_1`)&&w.warn(`glTF material uses ${r.join(`, `)} with TEXCOORD_1 but primitive is missing TEXCOORD_1; those textures will be skipped`)(),e.unlit||e.extensions?.KHR_materials_unlit||Wb(t,`NORMAL`))return;let i=e.normalTexture?`lit PBR shading with normalTexture`:`lit PBR shading`;w.warn(`glTF primitive is missing NORMAL while using ${i}; shading will fall back to geometric normals`)()}function Ub(e,t){let n=[];for(let r of kb()){let i=Gb(e,r.pathSegments);i&&Mb(i)===t&&n.push(r.displayName)}return n}function Wb(e,t){return Bb[t].some(t=>!!e[t])}function Gb(e,t){let n=e;for(let e of t)if(n=n?.[e],!n)return null;return n}function Kb(e,t,n,r,i){if(n.uniforms.unlit=!!(t.unlit||t.extensions?.KHR_materials_unlit),t.pbrMetallicRoughness&&Yb(e,t.pbrMetallicRoughness,n,r,i),t.normalTexture){G(e,t.normalTexture,`pbr_normalSampler`,n,{featureOptions:{define:`HAS_NORMALMAP`,enabledUniformName:`normalMapEnabled`},gltf:i,attributes:r,textureTransformSlot:`normal`});let{scale:a=1}=t.normalTexture;n.uniforms.normalScale=a}if(t.occlusionTexture){G(e,t.occlusionTexture,`pbr_occlusionSampler`,n,{featureOptions:{define:`HAS_OCCLUSIONMAP`,enabledUniformName:`occlusionMapEnabled`},gltf:i,attributes:r,textureTransformSlot:`occlusion`});let{strength:a=1}=t.occlusionTexture;n.uniforms.occlusionStrength=a}switch(n.uniforms.emissiveFactor=t.emissiveFactor||[0,0,0],t.emissiveTexture&&G(e,t.emissiveTexture,`pbr_emissiveSampler`,n,{featureOptions:{define:`HAS_EMISSIVEMAP`,enabledUniformName:`emissiveMapEnabled`},gltf:i,attributes:r,textureTransformSlot:`emissive`}),Xb(e,t.extensions,n,i,r),t.alphaMode||`OPAQUE`){case`OPAQUE`:break;case`MASK`:{let{alphaCutoff:e=.5}=t;n.defines.ALPHA_CUTOFF=!0,n.uniforms.alphaCutoffEnabled=!0,n.uniforms.alphaCutoff=e;break}case`BLEND`:w.warn(`glTF BLEND alphaMode might not work well because it requires mesh sorting`)(),qb(n);break}}function qb(e){e.parameters.blend=!0,e.parameters.blendColorOperation=`add`,e.parameters.blendColorSrcFactor=`src-alpha`,e.parameters.blendColorDstFactor=`one-minus-src-alpha`,e.parameters.blendAlphaOperation=`add`,e.parameters.blendAlphaSrcFactor=`one`,e.parameters.blendAlphaDstFactor=`one-minus-src-alpha`,e.glParameters.blend=!0,e.glParameters.blendEquation=U.FUNC_ADD,e.glParameters.blendFunc=[U.SRC_ALPHA,U.ONE_MINUS_SRC_ALPHA,U.ONE,U.ONE_MINUS_SRC_ALPHA]}function Jb(e){e.parameters.blend=!0,e.parameters.depthWriteEnabled=!1,e.parameters.blendColorOperation=`add`,e.parameters.blendColorSrcFactor=`one`,e.parameters.blendColorDstFactor=`one-minus-src-alpha`,e.parameters.blendAlphaOperation=`add`,e.parameters.blendAlphaSrcFactor=`one`,e.parameters.blendAlphaDstFactor=`one-minus-src-alpha`,e.glParameters.blend=!0,e.glParameters.depthMask=!1,e.glParameters.blendEquation=U.FUNC_ADD,e.glParameters.blendFunc=[U.ONE,U.ONE_MINUS_SRC_ALPHA,U.ONE,U.ONE_MINUS_SRC_ALPHA]}function Yb(e,t,n,r,i){t.baseColorTexture&&G(e,t.baseColorTexture,`pbr_baseColorSampler`,n,{featureOptions:{define:`HAS_BASECOLORMAP`,enabledUniformName:`baseColorMapEnabled`},gltf:i,attributes:r,textureTransformSlot:`baseColor`}),n.uniforms.baseColorFactor=t.baseColorFactor||[1,1,1,1],t.metallicRoughnessTexture&&G(e,t.metallicRoughnessTexture,`pbr_metallicRoughnessSampler`,n,{featureOptions:{define:`HAS_METALROUGHNESSMAP`,enabledUniformName:`metallicRoughnessMapEnabled`},gltf:i,attributes:r,textureTransformSlot:`metallicRoughness`});let{metallicFactor:a=1,roughnessFactor:o=1}=t;n.uniforms.metallicRoughnessValues=[a,o]}function Xb(e,t,n,r,i={}){t&&(Zb(t)&&(n.defines.USE_MATERIAL_EXTENSIONS=!0),Qb(e,t.KHR_materials_specular,n,r,i),$b(t.KHR_materials_ior,n),tx(e,t.EXT_materials_bump,n,r,i),ex(e,t.KHR_materials_transmission,n,r,i),nx(e,t.KHR_materials_diffuse_transmission,n,r,i),rx(e,t.KHR_materials_volume,n,r,i),ix(e,t.KHR_materials_volume_scatter,t.KHR_materials_volume,n,r,i),ax(t.KHR_materials_dispersion,n),ox(e,t.KHR_materials_clearcoat,n,r,i),sx(e,t.KHR_materials_sheen,n,r,i),cx(e,t.KHR_materials_iridescence,n,r,i),lx(e,t.KHR_materials_anisotropy,n,r,i),ux(t.KHR_materials_emissive_strength,n))}function Zb(e){return!!(e.KHR_materials_specular||e.KHR_materials_ior||e.EXT_materials_bump||e.KHR_materials_transmission||e.KHR_materials_diffuse_transmission||e.KHR_materials_volume||e.KHR_materials_volume_scatter||e.KHR_materials_dispersion||e.KHR_materials_clearcoat||e.KHR_materials_sheen||e.KHR_materials_iridescence||e.KHR_materials_anisotropy)}function Qb(e,t,n,r,i={}){t&&(t.specularColorFactor&&(n.uniforms.specularColorFactor=t.specularColorFactor),t.specularFactor!==void 0&&(n.uniforms.specularIntensityFactor=t.specularFactor),t.specularColorTexture&&G(e,t.specularColorTexture,`pbr_specularColorSampler`,n,{featureOptions:{define:`HAS_SPECULARCOLORMAP`,enabledUniformName:`specularColorMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`specularColor`}),t.specularTexture&&G(e,t.specularTexture,`pbr_specularIntensitySampler`,n,{featureOptions:{define:`HAS_SPECULARINTENSITYMAP`,enabledUniformName:`specularIntensityMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`specularIntensity`}))}function $b(e,t){e?.ior!==void 0&&(t.uniforms.ior=e.ior)}function ex(e,t,n,r,i={}){t&&(t.transmissionFactor!==void 0&&(n.uniforms.transmissionFactor=t.transmissionFactor),t.transmissionTexture&&G(e,t.transmissionTexture,`pbr_transmissionSampler`,n,{featureOptions:{define:`HAS_TRANSMISSIONMAP`,enabledUniformName:`transmissionMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`transmission`}),((t.transmissionFactor??0)>0||t.transmissionTexture)&&(w.warn(`KHR_materials_transmission uses a premultiplied-alpha blending approximation and may require mesh sorting`)(),Jb(n)))}function tx(e,t,n,r,i={}){t&&(n.uniforms.bumpFactor=Math.max(t.bumpFactor??1,0),t.bumpTexture&&G(e,t.bumpTexture,`pbr_bumpSampler`,n,{featureOptions:{define:`HAS_BUMPMAP`,enabledUniformName:`bumpMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`bump`}))}function nx(e,t,n,r,i={}){t&&(n.uniforms.diffuseTransmissionFactor=Math.min(Math.max(t.diffuseTransmissionFactor??0,0),1),n.uniforms.diffuseTransmissionColorFactor=t.diffuseTransmissionColorFactor||[1,1,1],t.diffuseTransmissionTexture&&G(e,t.diffuseTransmissionTexture,`pbr_diffuseTransmissionSampler`,n,{featureOptions:{define:`HAS_DIFFUSETRANSMISSIONMAP`,enabledUniformName:`diffuseTransmissionMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`diffuseTransmission`}),t.diffuseTransmissionColorTexture&&G(e,t.diffuseTransmissionColorTexture,`pbr_diffuseTransmissionColorSampler`,n,{featureOptions:{define:`HAS_DIFFUSETRANSMISSIONCOLORMAP`,enabledUniformName:`diffuseTransmissionColorMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`diffuseTransmissionColor`}))}function rx(e,t,n,r,i={}){t&&(t.thicknessFactor!==void 0&&(n.uniforms.thicknessFactor=t.thicknessFactor),t.thicknessTexture&&G(e,t.thicknessTexture,`pbr_thicknessSampler`,n,{featureOptions:{define:`HAS_THICKNESSMAP`},gltf:r,attributes:i,textureTransformSlot:`thickness`}),t.attenuationDistance!==void 0&&(n.uniforms.attenuationDistance=t.attenuationDistance),t.attenuationColor&&(n.uniforms.attenuationColor=t.attenuationColor))}function ix(e,t,n,r,i,a={}){!t||!n||(r.uniforms.multiscatterColorFactor=t.multiscatterColorFactor||t.multiscatterColor||[0,0,0],r.uniforms.scatterAnisotropy=Math.min(Math.max(t.scatterAnisotropy??0,-.999),.999),t.multiscatterColorTexture&&G(e,t.multiscatterColorTexture,`pbr_multiscatterColorSampler`,r,{featureOptions:{define:`HAS_MULTISCATTERCOLORMAP`,enabledUniformName:`multiscatterColorMapEnabled`},gltf:i,attributes:a,textureTransformSlot:`multiscatterColor`}))}function ax(e,t){e?.dispersion!==void 0&&(t.uniforms.dispersion=Math.max(e.dispersion,0))}function ox(e,t,n,r,i={}){t&&(t.clearcoatFactor!==void 0&&(n.uniforms.clearcoatFactor=t.clearcoatFactor),t.clearcoatRoughnessFactor!==void 0&&(n.uniforms.clearcoatRoughnessFactor=t.clearcoatRoughnessFactor),t.clearcoatTexture&&G(e,t.clearcoatTexture,`pbr_clearcoatSampler`,n,{featureOptions:{define:`HAS_CLEARCOATMAP`,enabledUniformName:`clearcoatMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`clearcoat`}),t.clearcoatRoughnessTexture&&G(e,t.clearcoatRoughnessTexture,`pbr_clearcoatRoughnessSampler`,n,{featureOptions:{define:`HAS_CLEARCOATROUGHNESSMAP`,enabledUniformName:`clearcoatRoughnessMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`clearcoatRoughness`}),t.clearcoatNormalTexture&&G(e,t.clearcoatNormalTexture,`pbr_clearcoatNormalSampler`,n,{featureOptions:{define:`HAS_CLEARCOATNORMALMAP`},gltf:r,attributes:i,textureTransformSlot:`clearcoatNormal`}))}function sx(e,t,n,r,i={}){t&&(t.sheenColorFactor&&(n.uniforms.sheenColorFactor=t.sheenColorFactor),t.sheenRoughnessFactor!==void 0&&(n.uniforms.sheenRoughnessFactor=t.sheenRoughnessFactor),t.sheenColorTexture&&G(e,t.sheenColorTexture,`pbr_sheenColorSampler`,n,{featureOptions:{define:`HAS_SHEENCOLORMAP`,enabledUniformName:`sheenColorMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`sheenColor`}),t.sheenRoughnessTexture&&G(e,t.sheenRoughnessTexture,`pbr_sheenRoughnessSampler`,n,{featureOptions:{define:`HAS_SHEENROUGHNESSMAP`,enabledUniformName:`sheenRoughnessMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`sheenRoughness`}))}function cx(e,t,n,r,i={}){t&&(t.iridescenceFactor!==void 0&&(n.uniforms.iridescenceFactor=t.iridescenceFactor),t.iridescenceIor!==void 0&&(n.uniforms.iridescenceIor=t.iridescenceIor),(t.iridescenceThicknessMinimum!==void 0||t.iridescenceThicknessMaximum!==void 0)&&(n.uniforms.iridescenceThicknessRange=[t.iridescenceThicknessMinimum??100,t.iridescenceThicknessMaximum??400]),t.iridescenceTexture&&G(e,t.iridescenceTexture,`pbr_iridescenceSampler`,n,{featureOptions:{define:`HAS_IRIDESCENCEMAP`,enabledUniformName:`iridescenceMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`iridescence`}),t.iridescenceThicknessTexture&&G(e,t.iridescenceThicknessTexture,`pbr_iridescenceThicknessSampler`,n,{featureOptions:{define:`HAS_IRIDESCENCETHICKNESSMAP`},gltf:r,attributes:i,textureTransformSlot:`iridescenceThickness`}))}function lx(e,t,n,r,i={}){t&&(t.anisotropyStrength!==void 0&&(n.uniforms.anisotropyStrength=t.anisotropyStrength),t.anisotropyRotation!==void 0&&(n.uniforms.anisotropyRotation=t.anisotropyRotation),t.anisotropyTexture&&G(e,t.anisotropyTexture,`pbr_anisotropySampler`,n,{featureOptions:{define:`HAS_ANISOTROPYMAP`,enabledUniformName:`anisotropyMapEnabled`},gltf:r,attributes:i,textureTransformSlot:`anisotropy`}))}function ux(e,t){e?.emissiveStrength!==void 0&&(t.uniforms.emissiveStrength=e.emissiveStrength)}function G(e,t,n,r,i={}){let{featureOptions:a={},gltf:o,attributes:s={},textureTransformSlot:c}=i,{define:l,enabledUniformName:u}=a,d=Mb(t);if(d>1){w.warn(`Skipping ${String(n)} because ${d} is not supported; only TEXCOORD_0 and TEXCOORD_1 are currently available`)();return}if(d===1&&!Wb(s,`TEXCOORD_1`)){w.warn(`Skipping ${String(n)} because it requires TEXCOORD_1 but the primitive does not provide TEXCOORD_1`)();return}let f=fx(t,o),p=f.texture?.source?.image;if(!p){w.warn(`Skipping unresolved glTF texture for ${String(n)}`)();return}let m=dx(e,p,{id:f.uniformName||f.id,sampler:{addressModeU:`repeat`,addressModeV:`repeat`,minFilter:`linear`,magFilter:`linear`,...Ib(f.texture.sampler)}});if(r.bindings[n]=m,l&&(r.defines[l]=!0),u&&(r.uniforms[u]=!0),c){let e=Ab(c);r.uniforms[e.uvSetUniform]=d,r.uniforms[e.uvTransformUniform]=Pb(jb(t))}r.generatedTextures.push(m)}function dx(e,t,n){if(`compressed`in t)return gx(e,t,{id:n.id,sampler:n.sampler});let r=n.width!==void 0&&n.height!==void 0?{width:n.width,height:n.height}:e.getExternalImageSize(t),i=n.sampler.mipmapFilter===`nearest`||n.sampler.mipmapFilter===`linear`,a=i?e.getMipLevelCount(r.width,r.height):1,o=e.createTexture({id:n.id,sampler:n.sampler,width:r.width,height:r.height,mipLevels:a,...i?{usage:M.SAMPLE|M.RENDER|M.COPY_DST|M.COPY_SRC}:{},...n.colorSpace?{format:n.colorSpace===`srgb`?`rgba8unorm-srgb`:`rgba8unorm`}:{},data:t});return a>1&&(e.type===`webgl`?o.generateMipmapsWebGL():e.type===`webgpu`&&e.generateMipmapsWebGPU(o)),o}function fx(e,t){if(e.texture||e.index===void 0||!t?.textures)return e;let n=t.textures[e.index];return n?`texture`in n&&n.texture?{...n,...e,texture:n.texture}:`source`in n?{...e,texture:n}:e:e}function px(e,t){return e.createTexture({...t,format:`rgba8unorm`,width:1,height:1,mipLevels:1})}function mx(e){return e.textureFormat}function hx(e,t,n){let{blockWidth:r=1,blockHeight:i=1}=oe.getInfo(n),a=1;for(let n=1;;n++){let o=Math.max(1,e>>n),s=Math.max(1,t>>n);if(o<r||s<i)break;a++}return a}function gx(e,t,n){let r;if(r=Array.isArray(t.data)&&t.data[0]?.data?t.data:`mipmaps`in t&&Array.isArray(t.mipmaps)?t.mipmaps:[],r.length===0||!r[0]?.data)return w.warn(`createCompressedTexture: compressed image has no valid mip levels, creating fallback`)(),px(e,n);let i=r[0],a=i.width??t.width??0,o=i.height??t.height??0;if(a<=0||o<=0)return w.warn(`createCompressedTexture: base level has invalid dimensions, creating fallback`)(),px(e,n);let s=mx(i);if(!s)return w.warn(`createCompressedTexture: compressed image has no textureFormat, creating fallback`)(),px(e,n);if(!e.isTextureFormatSupported(s))return w.warn(`createCompressedTexture: ${e.type} device does not support '${s}', creating fallback`)(),px(e,n);let c=hx(a,o,s),l=Math.min(r.length,c),u=1;for(let e=1;e<l;e++){let t=r[e];if(!t.data||t.width<=0||t.height<=0){w.warn(`createCompressedTexture: mip level ${e} has invalid data/dimensions, truncating`)();break}let n=mx(t);if(n&&n!==s){w.warn(`createCompressedTexture: mip level ${e} format '${n}' differs from base '${s}', truncating`)();break}let i=Math.max(1,a>>e),c=Math.max(1,o>>e);if(t.width!==i||t.height!==c){w.warn(`createCompressedTexture: mip level ${e} dimensions ${t.width}x${t.height} don't match expected ${i}x${c}, truncating`)();break}u++}let d=e.createTexture({...n,format:s,usage:M.TEXTURE|M.COPY_DST,width:a,height:o,mipLevels:u,data:i.data});for(let e=1;e<u;e++)d.writeData(r[e].data,{width:r[e].width,height:r[e].height,mipLevel:e});return d}var _x={modelOptions:{},pbrDebug:!1,imageBasedLightingEnvironment:void 0,lights:!0,useTangents:!1,useByteColors:!0,strictExtensions:!1};function vx(e,t,n={}){let r=new Set,i={..._x,...n,generatedTextures:r},a=new Di(e,{modules:[Qr]}),o=(t.materials||[]).map((n,r)=>gb(e,{id:Ex(n,r),parsedPPBRMaterial:yx(e,n,{},{...i,gltf:t,validateAttributes:!1}),materialFactory:a})),s=new Map;(t.materials||[]).forEach((e,t)=>{s.set(e.id,o[t])});let c=new Map;t.meshes.forEach((n,r)=>{let a=xx(e,n,t,s,i);c.set(n.id,a)});let l=new Map,u=new Map,d=new Set,f=new Set,p=new Set;return t.nodes.forEach((t,n)=>{let r=bx(e,t,i);l.set(n,r),u.set(t.id,r)}),t.nodes.forEach((n,r)=>{if(l.get(r).add((n.children??[]).map(({id:e})=>{let t=u.get(e);if(!t)throw Error(`Cannot find child ${e} of node ${r}`);return t})),n.mesh){let a=n.mesh,o=xb(t,n),u=a.primitives.some(e=>!!e.targets?.length),m=o||u&&d.has(a.id)?xx(e,a,t,s,i,o||void 0):c.get(a.id);if(!m)throw Error(`Cannot find mesh child ${n.mesh.id} of node ${r}`);let h=l.get(r),g=c.get(a.id),_=f.has(a.id)&&(n.skin!==void 0||p.has(a.id))&&m===g?xx(e,a,t,s,i):m;if(h.add(_),h.userData.gltfMesh=_,f.add(a.id),n.skin!==void 0&&p.add(a.id),u){d.add(a.id);let e=a.primitives.find(e=>e.targets?.length)?.targets?.length||0,t=n.weights||a.weights||Array(e).fill(0);h.userData.morphMeshes=[_],i.modelOptions?.userData?.gltfAnimatedCrowd?h.userData.morphWeights=[...t]:wb(h,t)}}}),{scenes:t.scenes.map(e=>{let t=(e.nodes||[]).map(({id:t})=>{let n=u.get(t);if(!n)throw Error(`Cannot find child ${t} of scene ${e.name||e.id}`);return n});return new sa({id:e.name||e.id,children:t})}),materials:o,gltfMeshIdToNodeMap:c,gltfNodeIdToNodeMap:u,gltfNodeIndexToNodeMap:l,generatedTextures:r}}function yx(e,t,n,r){let i=Vb(e,t,n,r);for(let e of i.generatedTextures)r.generatedTextures.add(e);return i}function bx(e,t,n){return new sa({id:t.name||t.id,children:[],matrix:t.matrix,display:t.extensions?.KHR_node_visibility?.visible!==!1,position:t.translation,rotation:t.rotation,scale:t.scale})}function xx(e,t,n,r,i,a){let o=(t.primitives||[]).map((o,s)=>Sx({device:e,gltfPrimitive:o,primitiveIndex:s,gltfMesh:t,gltf:n,gltfMaterialIdToMaterialMap:r,options:i,instancing:a}));return new sa({id:t.name||t.id,children:o})}function Sx({device:e,gltfPrimitive:t,primitiveIndex:n,gltfMesh:r,gltf:i,gltfMaterialIdToMaterialMap:a,options:o,instancing:s}){let c=t.name||`${r.name||r.id}-primitive-${n}`,l=wx(t.attributes),u=Tx(c,t,Eb(t.mode??4,t.indices?.value,l)),d=u.vertexCount,f=Cx(t,i,u),p=yx(e,t.material,u.attributes,{...o,gltf:i}),m=_b(e,{id:c,geometry:u,material:t.material&&a.get(t.material.id)||null,parsedPPBRMaterial:p,modelOptions:o.modelOptions,vertexCount:d,bounds:[t.attributes.POSITION.min,t.attributes.POSITION.max],instanceMatrices:s?.matrices,morphTargets:f?.targets});s&&(m.userData.gltfInstancing=s);let h=t.extensions?.KHR_materials_variants?.mappings||[];if(h.length){let t=new Map;for(let n of h){let r=typeof n.material==`number`?i.materials[n.material]:n.material,s=r&&a.get(r.id);if(!s)continue;let c=yx(e,r,u.attributes,{...o,gltf:i});for(let e of n.variants||[])t.set(e,{material:s,parameters:{...m.model.parameters,...c.parameters,depthWriteEnabled:r.alphaMode!==`BLEND`,cullMode:r.doubleSided?`none`:`back`}})}m.userData.gltfMaterialVariants={defaultMaterial:m.model.material,defaultParameters:{...m.model.parameters},mappings:t}}return f&&(m.userData.morphTargets=f),m}function Cx(e,t,n){if(!e.targets?.length)return;let r={};for(let e of[`POSITION`,`NORMAL`,`TANGENT`]){let t=n.attributes[e]?.value;t instanceof Float32Array&&(r[e]=new Float32Array(t))}return{geometry:n,baseAttributes:r,targets:e.targets.map(e=>{let n={};for(let r of[`POSITION`,`NORMAL`,`TANGENT`]){let i=e[r],a=typeof i==`number`?t.accessors[i]:i;a?.value&&ArrayBuffer.isView(a.value)&&(n[r]=di(a))}return n})}}function wx(e){let t=1/0;for(let n of Object.values(e))if(n){let{value:e,size:r,components:i}=n,a=r??i;e?.length!==void 0&&a>=1&&(t=Math.min(t,e.length/a))}if(!Number.isFinite(t))throw Error(`Could not determine vertex count from attributes`);return t}function Tx(e,t,n){let r={};for(let[e,n]of Object.entries(t.attributes)){let{components:i,size:a,value:o,normalized:s}=n,c=e===`POSITION`||e===`NORMAL`||e===`TANGENT`,l=!!(t.targets?.length&&c);r[e]={size:a??i,value:l?di({value:o,normalized:s}):o,normalized:!l&&s}}return new de({id:e,topology:n.topology,indices:n.indices??t.indices?.value,attributes:r})}function Ex(e,t){return e.name||e.id||`material-${t}`}function Dx(e,t={}){let n=t.lightDefinitions||e.lights||e.extensions?.KHR_lights_punctual?.lights;if(!n||!Array.isArray(n)||n.length===0)return[];let r=[],i=Nx(e.nodes||[]),a=new Map;for(let o of e.nodes||[]){if(!Ox(o,i,t.nodeVisibility))continue;let e=o.light??o.extensions?.KHR_lights_punctual?.light;if(typeof e!=`number`||t.nodeIdentifiers&&!t.nodeIdentifiers.has(o.id))continue;let s=n[e];if(!s)continue;let c=kx(s.color||[1,1,1],t.useByteColors??!0),l=s.intensity??1,u=s.range,d=Px(o,i,a);switch(s.type){case`directional`:r.push(jx(d,c,l));break;case`point`:r.push(Ax(d,c,l,u));break;case`spot`:r.push(Mx(d,c,l,u,s.spot));break;default:break}}return r}function Ox(e,t,n){let r=e;for(;r;){let e=n?.get(r.id);if(e?!e.display:r.extensions?.KHR_node_visibility?.visible===!1)return!1;r=t.get(r.id)}return!0}function kx(e,t){return t?e.map(e=>e*255):ur(e,!1)}function Ax(e,t,n,r){let i=Ix(e),a=[1,0,0];return r!==void 0&&r>0&&(a=[1,0,1/(r*r)]),{type:`point`,position:i,color:t,intensity:n,attenuation:a}}function jx(e,t,n){return{type:`directional`,direction:Lx(e),color:t,intensity:n}}function Mx(e,t,n,r,i={}){let a=Ix(e),o=Lx(e),s=[1,0,0];return r!==void 0&&r>0&&(s=[1,0,1/(r*r)]),{type:`spot`,position:a,direction:o,color:t,intensity:n,attenuation:s,innerConeAngle:i.innerConeAngle??0,outerConeAngle:i.outerConeAngle??Math.PI/4}}function Nx(e){let t=new Map;for(let n of e)for(let e of n.children||[])t.set(e.id,n);return t}function Px(e,t,n){let r=n.get(e.id);if(r)return r;let i=Fx(e),a=t.get(e.id),o=a?new R(Px(a,t,n)).multiplyRight(i):i;return n.set(e.id,o),o}function Fx(e){if(e.matrix)return new R(e.matrix);let t=new R;return e.translation&&t.translate(e.translation),e.rotation&&t.multiplyRight(new R().fromQuaternion(e.rotation)),e.scale&&t.scale(e.scale),t}function Ix(e){return e.transformAsPoint([0,0,0])}function Lx(e){return e.transformDirection([0,0,-1])}var Rx=class extends $r{animation;gltfNodeIdToNodeMap;onVisibilityChange;cameras;lightDefinitions;onLightChange;materials;clip;mixer;action;materialTextureTransformState=new Map;constructor(e){if(super({name:e.animation.name||`unnamed`}),this.animation=e.animation,this.gltfNodeIdToNodeMap=e.gltfNodeIdToNodeMap,this.onVisibilityChange=e.onVisibilityChange,this.cameras=e.cameras||[],this.lightDefinitions=e.lightDefinitions||[],this.onLightChange=e.onLightChange,this.materials=e.materials||[],this.animation.name||=`unnamed`,this.name=this.animation.name,this.animation.channels.some(e=>e.type===`material`||e.type===`textureTransform`)&&!this.materials.length)throw Error(`Animation ${this.animation.name} targets materials, but GLTFAnimator was created without a materials array`);this.mixer=e.mixer||new ui,this.clip=new ci({name:this.name,tracks:this.animation.channels.map(e=>this.createAnimationTrack(e))}),this.action=this.mixer.clipAction(this.clip).play()}applyTime(e){this.action.setTime(e),this.mixer.update(0)}createAnimationTrack(e){let t=Bx(e.sampler.interpolation);if(e.type===`node`)return new si({name:`${e.targetNodeId}.${e.path}`,times:e.sampler.input,values:e.sampler.output,interpolation:t,valueType:e.path===`rotation`?`quaternion`:`vector`,binding:{id:`node:${e.targetNodeId}:${e.path}`,getValue:()=>this.getNodeAnimationValue(e.targetNodeId,e.path),setValue:t=>this.applyNodeAnimationValue(e.targetNodeId,e.path,t)}});if(e.type===`camera`||e.type===`light`)return new si({name:e.pointer,times:e.sampler.input,values:e.sampler.output,interpolation:t,binding:{id:e.pointer,getValue:()=>this.getSceneAnimationValue(e),setValue:t=>this.applySceneAnimationValue(e,t)}});let n=this.materials[e.targetMaterialIndex];if(!n)throw Error(`Cannot find animation target material ${e.targetMaterialIndex} for ${e.pointer}`);return new si({name:e.pointer,times:e.sampler.input,values:e.sampler.output,interpolation:t,binding:{id:e.pointer,getValue:e.type===`material`?()=>Vx(n,e):void 0,setValue:t=>{e.type===`material`?Hx(n,e,t):Gx(n,e,t,this.materialTextureTransformState)}}})}getNodeAnimationValue(e,t){let n=this.getTargetNode(e);switch(t){case`translation`:return Array.from(n.position);case`rotation`:return Array.from(n.rotation);case`scale`:return Array.from(n.scale);case`weights`:return Array.from(n.userData.morphWeights||[]);case`visibility`:return[+!!n.display];default:return[]}}applyNodeAnimationValue(e,t,n){let r=this.getTargetNode(e);switch(t){case`translation`:r.setPosition(n).updateMatrix();break;case`rotation`:r.setRotation(n).updateMatrix();break;case`scale`:r.setScale(n).updateMatrix();break;case`weights`:wb(r,n);break;case`visibility`:r.setProps({display:n[0]!==0}),this.onVisibilityChange?.();break;default:w.warn(`Bad animation path ${t}`)()}}getTargetNode(e){let t=this.gltfNodeIdToNodeMap.get(e);if(!t)throw Error(`Cannot find animation target node ${e}`);return t}getSceneAnimationValue(e){if(e.type===`camera`){let t=this.cameras[e.targetCameraIndex]?.[e.projection]?.[e.property];return typeof t==`number`?[t]:[]}let t=this.lightDefinitions[e.targetLightIndex],n=e.property===`innerConeAngle`||e.property===`outerConeAngle`?t?.spot?.[e.property]:t?.[e.property];return Array.isArray(n)?e.component===void 0?[...n]:[n[e.component]]:typeof n==`number`?[n]:[]}applySceneAnimationValue(e,t){if(e.type===`camera`){let n=this.cameras[e.targetCameraIndex];n?.[e.projection]&&(n[e.projection][e.property]=t[0]);return}let n=this.lightDefinitions[e.targetLightIndex];if(n){if(e.property===`innerConeAngle`||e.property===`outerConeAngle`)n.spot||={},n.spot[e.property]=t[0];else if(e.component!==void 0){let r=[...n[e.property]||[1,1,1]];r[e.component]=t[0],n[e.property]=r}else n[e.property]=t.length===1?t[0]:[...t];this.onLightChange?.()}}},zx=class extends ei{mixer;activeClip;onUpdate;previousTimeSeconds;constructor(e){let t=new ui;super(e.animations.map((n,r)=>{let i=n.name||`Animation-${r}`;return new Rx({gltfNodeIdToNodeMap:e.gltfNodeIdToNodeMap,onVisibilityChange:e.onVisibilityChange,cameras:e.cameras,lightDefinitions:e.lightDefinitions,onLightChange:e.onLightChange,materials:e.materials,mixer:t,animation:{name:i,channels:n.channels}})})),this.mixer=t,this.onUpdate=e.onUpdate,this.activeClip=this.clips[0]?.name,e.autoplay===!1?this.clips.forEach(e=>{e.playing=!1,e.action.stop()}):e.autoplay===`first`&&this.activeClip&&this.selectClip(this.activeClip)}setUpdateHandler(e){return this.onUpdate=e,this}setTime(e){let t=e/1e3,n=this.previousTimeSeconds===void 0?0:t-this.previousTimeSeconds;this.previousTimeSeconds=t;let r=n*this.mixer.timeScale;this.clips.forEach(e=>{if(!e.playing){e.action.stop();return}if(e.action.paused)return;e.action.resume();let n=Math.max(0,t-e.startTime)*e.speed;e.action.setTime(n-r*e.action.timeScale)}),this.mixer.update(n),this.onUpdate?.()}update(e){this.mixer.update(e),this.onUpdate?.()}selectClip(e,t={}){let n=this.clips.find(t=>t.name===e);if(!n)throw Error(`Unknown animation clip: ${e}`);let r=this.clips.find(e=>e.name===this.activeClip),i=t.crossFadeDuration||0;for(let e of this.clips)e!==n&&!(i>0&&e===r)&&(e.playing=!1,e.action.stop());return n.playing=!0,i>0&&r&&r!==n?(r.playing=!0,r.action.crossFadeTo(n.action,i)):n.action.reset().setEffectiveWeight(1).play(),this.activeClip=e,n}};function Bx(e){switch(e){case`STEP`:case`LINEAR`:case`CUBICSPLINE`:return e;default:throw Error(`Unsupported animation interpolation: ${e}`)}}function Vx(e,t){let n=e.shaderInputs.getUniformValues().pbrMaterial?.[t.property];return Array.isArray(n)?t.component===void 0?[...n]:[n[t.component]]:typeof n==`number`?[n]:[]}function Hx(e,t,n){let r=t.component===void 0?{[t.property]:n.length===1?n[0]:n}:{[t.property]:Wx(Ux(e,t.property),t.component,n[0])};e.setProps({pbrMaterial:r})}function Ux(e,t){let n=e.shaderInputs.getUniformValues().pbrMaterial?.[t];return Array.isArray(n)?[...n]:[]}function Wx(e,t,n){let r=[...e];return r[t]=n,r}function Gx(e,t,n,r){let i=Ab(t.textureSlot),a=Kx(r,e,t);switch(t.path){case`offset`:t.component===void 0?a.offset=[n[0],n[1]]:a.offset[t.component]=n[0];break;case`rotation`:a.rotation=n[0];break;case`scale`:t.component===void 0?a.scale=[n[0],n[1]]:a.scale[t.component]=n[0];break}e.setProps({pbrMaterial:{[i.uvTransformUniform]:Fb(t.baseTransform,a)}})}function Kx(e,t,n){let r=e.get(t)||{},i=r[n.textureSlot];return i||(i={offset:[...n.baseTransform.offset],rotation:n.baseTransform.rotation,scale:[...n.baseTransform.scale]},r[n.textureSlot]=i,e.set(t,r)),i}var qx=class{bindings;scenes;constructor(e){this.scenes=e.scenes,this.bindings=Jx(e),this.update()}update(){if(this.bindings.length===0)return;let e=new Map;for(let t of this.scenes)t.preorderTraversal((t,{worldMatrix:n})=>{t instanceof sa&&e.set(t,new R(n))});for(let t of this.bindings){_i({joints:t.joints,meshNode:t.node,worldMatrices:e,inverseBindMatrices:t.inverseBindMatrices,target:t.jointMatrices});for(let e of t.models)e.model.shaderInputs.setProps({skin:{jointMatrices:t.jointMatrices}})}}getBinding(e){return this.bindings.find(t=>typeof e==`number`?t.nodeIndex===e:t.node===e)}};function Jx(e){let{gltf:t,gltfNodeIndexToNodeMap:n}=e,r=[],i=t.skins||[],a=new Set;for(let t of e.scenes)t.preorderTraversal(e=>{e instanceof sa&&a.add(e)});for(let[e,o]of t.nodes.entries()){let s=o.skin;if(s===void 0||!o.mesh)continue;let c=Yx(t,s),l=i[c],u=n.get(e);if(!l||!u||!a.has(u))continue;let d=l.joints.flatMap(e=>{let t=n.get(e);return t?[t]:[]});if(d.length!==l.joints.length)continue;let f=o.mesh,p=u.userData.gltfMesh,m=p instanceof sa?p:u.children.find(e=>e instanceof sa&&e.id===(f.name||f.id));if(!(m instanceof sa))continue;let h=m.children.flatMap(e=>e instanceof ca?[e]:[]),g=l.inverseBindMatrices?.value;r.push({nodeIndex:e,skinIndex:c,node:u,joints:d,...g instanceof Float32Array?{inverseBindMatrices:g}:{},jointMatrices:new Float32Array(d.length*16),models:h})}return r}function Yx(e,t){return typeof t==`number`?t:(e.skins||[]).findIndex(n=>{if(n===t||t.id&&n.id===t.id)return!0;if(n.joints.length!==t.joints?.length||!n.joints.every((e,n)=>e===t.joints?.[n]))return!1;if(typeof t.inverseBindMatrices==`number`){let r=e.accessors[t.inverseBindMatrices];return!n.inverseBindMatrices||n.inverseBindMatrices===r}return!0})}var Xx={supportLevel:`none`,standardStatus:`unknown`,comment:`Not currently listed in the luma.gl glTF extension support registry.`},Zx={KHR_draco_mesh_compression:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`Decoded by loaders.gl before luma.gl builds the scenegraph.`},EXT_meshopt_compression:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`EXT meshopt-compressed buffer views are decoded by loaders.gl before rendering.`},KHR_meshopt_compression:{supportLevel:`none`,standardStatus:`release-candidate`,comment:`The installed loaders.gl GLTFLoader supports EXT_meshopt_compression, not the KHR release candidate.`},KHR_mesh_quantization:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`Loader-materialized quantized accessors retain their typed values and normalization.`},EXT_mesh_features:{supportLevel:`loader-only`,standardStatus:`ratified`,comment:`Feature identifiers are decoded by loaders.gl; automatic rendering and picking are application-owned.`},EXT_structural_metadata:{supportLevel:`loader-only`,standardStatus:`ratified`,comment:`Structural metadata is decoded by loaders.gl; automatic rendering and querying are application-owned.`},KHR_lights_punctual:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`Parsed into luma.gl Light objects.`},KHR_materials_unlit:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`Unlit materials bypass the default lighting path.`},KHR_materials_emissive_strength:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`Applied by the stock PBR shader.`},KHR_texture_basisu:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`BasisU / KTX2 textures pass through when the device supports them.`},KHR_texture_transform:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`Per-slot UV transforms and animated pointers are applied at runtime; avoid duplicate legacy loader-side baking.`},EXT_texture_webp:{supportLevel:`loader-only`,standardStatus:`ratified`,comment:`Texture source is resolved during load; final support depends on browser and device decode support.`},EXT_texture_avif:{supportLevel:`none`,standardStatus:`ratified`,comment:`The image loader can decode supported AVIF images, but GLTFLoader does not select EXT_texture_avif sources.`},KHR_materials_specular:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`The stock shader now applies specular factors and textures to the dielectric F0 term.`},KHR_materials_ior:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`The stock shader now drives dielectric reflectance from the glTF IOR value.`},KHR_materials_transmission:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`The stock shader now applies transmission to the base layer and exposes transparency through alpha, without a scene-color refraction buffer.`},KHR_materials_volume:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`Thickness and attenuation now tint transmitted light in the stock shader.`},KHR_materials_clearcoat:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`The stock shader now adds a secondary clearcoat specular lobe.`},KHR_materials_sheen:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`The stock shader now adds a sheen lobe for cloth-like materials.`},KHR_materials_iridescence:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`The stock shader now tints specular response with a view-dependent thin-film iridescence approximation.`},KHR_materials_anisotropy:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`The stock shader now shapes highlights and IBL response with an anisotropy-direction approximation.`},KHR_materials_pbrSpecularGlossiness:{supportLevel:`loader-only`,standardStatus:`archived`,comment:`Extension data can be loaded, but it is not translated into the default metallic-roughness material path.`},KHR_materials_variants:{supportLevel:`parsed-and-wired`,standardStatus:`ratified`,comment:`Primitive material variants can be selected and restored on the generated scenegraph.`},EXT_mesh_gpu_instancing:{supportLevel:`built-in`,standardStatus:`ratified`,comment:`Accessor-backed instance transforms use one instanced draw per source primitive.`},KHR_node_visibility:{supportLevel:`parsed-and-wired`,standardStatus:`ratified`,comment:`Recursive node visibility controls rendered geometry, punctual lights, and animation.`},KHR_animation_pointer:{supportLevel:`parsed-and-wired`,standardStatus:`ratified`,comment:`Node transforms, morph weights and visibility, material factors, texture transforms, camera projections, and punctual lights are wired to runtime updates.`},EXT_materials_bump:{supportLevel:`built-in`,standardStatus:`draft`,comment:`The experimental bump-map draft perturbs the canonical surface normal from a linear height texture.`},KHR_materials_diffuse_transmission:{supportLevel:`built-in`,standardStatus:`release-candidate`,comment:`The Khronos release candidate adds energy-conserving back-lit diffuse transmission and independent color/factor textures.`},KHR_materials_dispersion:{supportLevel:`parsed-and-wired`,standardStatus:`ratified`,comment:`The canonical PBR shader separates red, green, and blue transmission using wavelength-dependent refraction.`},KHR_materials_volume_scatter:{supportLevel:`parsed-and-wired`,standardStatus:`draft`,comment:`The unratified volume-scattering draft is approximated per surface; random-walk and screen-space diffusion are not implemented.`},KHR_xmp:{supportLevel:`none`,standardStatus:`archived`,comment:`Metadata payloads remain in the loaded glTF, but luma.gl does not interpret them.`},KHR_xmp_json_ld:{supportLevel:`none`,standardStatus:`ratified`,comment:`Metadata is preserved in the glTF, but luma.gl does not interpret it.`},EXT_lights_image_based:{supportLevel:`none`,standardStatus:`multi-vendor`,comment:`Use loadPBREnvironment() or custom environment setup instead.`},EXT_texture_video:{supportLevel:`none`,standardStatus:`multi-vendor`,comment:`Video textures are not created automatically by the stock pipeline.`},MSFT_lod:{supportLevel:`parsed-and-wired`,standardStatus:`vendor`,comment:`Node levels are parsed and selected by opt-in animated crowds; material LOD and GPU-driven selection are not implemented.`}};function Qx(e,t){let n=Array.from(iS(e)).sort(),r=new Set(e.extensionsRequired||[]),i=n.map(n=>{let i=Zx[n]||Xx,a=tS(n,i,e,t);return[n,{extensionName:n,required:r.has(n),supported:oS(a.supportLevel),supportLevel:a.supportLevel,standardStatus:i.standardStatus,comment:a.comment}]});return new Map(i)}function $x(e,t){return Array.from(Qx(e,t).values()).filter(e=>e.required&&!e.supported)}function eS(e,t){let n=$x(e,t);if(n.length)throw Error(`Unsupported required glTF extensions: ${n.map(e=>e.extensionName).join(`, `)}`)}function tS(e,t,n,r){if(e!==`KHR_texture_basisu`||!r)return t;let i=nS(n).find(e=>e===null||!r.isTextureFormatSupported(e));return i===void 0?t:{supportLevel:`none`,comment:i===null?`The ${r.type} device cannot use a BasisU texture whose transcoded GPU format is missing.`:`The ${r.type} device does not support the transcoded BasisU texture format '${i}'.`}}function nS(e){let t=[];for(let n of e.textures||[]){let e=n?.source?.image;if(!e?.compressed)continue;let r=Array.isArray(e.data)?e.data[0]:Array.isArray(e.mipmaps)?e.mipmaps[0]:void 0;t.push(r?.textureFormat??null)}return t}function rS(e){return Zx[e]||null}function iS(e){let t=e,n=new Set;return aS(n,e.extensionsUsed),aS(n,e.extensionsRequired),aS(n,t.extensionsRemoved),aS(n,Object.keys(e.extensions||{})),(t.lights?.length||(e.nodes||[]).some(e=>`light`in e))&&n.add(`KHR_lights_punctual`),(e.materials||[]).some(e=>{let t=e;return t.unlit||t.extensions?.KHR_materials_unlit})&&n.add(`KHR_materials_unlit`),n}function aS(e,t=[]){for(let n of t)e.add(n)}function oS(e){return e===`built-in`||e===`parsed-and-wired`}function sS(e){let t=e.animations||[],n=new Map,r=new Map;return t.flatMap((t,i)=>{let a=t.name||`Animation-${i}`,o=new Map,s=t.channels.flatMap(({sampler:i,target:a})=>{let s=pS(e,a),c=`${i}:${s??0}`,l=o.get(c);if(!l){let a=t.samplers[i];if(!a)throw Error(`Cannot find animation sampler ${i}`);let{input:u,interpolation:d=`LINEAR`,output:f}=a,p=wS(e.accessors[u],n),m=TS(e.accessors[f],r);l={input:p,interpolation:d,output:s===void 0?m:mS(m,p.length,d,s)},o.set(c,l)}let u=cS(e,a,l);return u?[u]:[]});return s.length?[{name:a,channels:s}]:[]})}function cS(e,t,n){if(t.path===`pointer`)return lS(e,t,n);let r=gS(t.path);if(!r)return null;let i=e.nodes[t.node??0];if(!i)throw Error(`Cannot find animation target ${t.node}`);return{type:`node`,sampler:n,targetNodeId:i.id,path:r}}function lS(e,t,n){let r=t.extensions?.KHR_animation_pointer?.pointer;if(typeof r!=`string`||!r.startsWith(`/`))return w.warn(`KHR_animation_pointer channel is missing a valid JSON pointer and will be skipped`)(),null;let i=bS(r);switch(i[0]){case`nodes`:return fS(e,i,n,r);case`materials`:return hS(e,i,n,r);case`cameras`:return uS(e,i,n,r);case`extensions`:if(i[1]===`KHR_lights_punctual`)return dS(e,i,n,r);break;default:break}return CS(r,`top-level target "${i[0]}" has no runtime animation mapping`),null}function uS(e,t,n,r){let i=Number(t[1]),a=e.cameras?.[i],o=t[2],s=t[3];return t.length!==4||!Number.isInteger(i)||!a||o!==`perspective`&&o!==`orthographic`||a.type!==o||!(o===`perspective`?[`aspectRatio`,`yfov`,`znear`,`zfar`]:[`xmag`,`ymag`,`znear`,`zfar`]).includes(s)?(CS(r,`camera pointers must target a supported projection property`),null):{type:`camera`,sampler:n,pointer:r,targetCameraIndex:i,projection:o,property:s}}function dS(e,t,n,r){let i=Number(t[3]),a=e.lights||e.extensions?.KHR_lights_punctual?.lights,o=t[4]===`spot`,s=o?t[5]:t[4],c=!o&&s===`color`?t[5]:void 0,l=[`color`,`intensity`,`range`,`innerConeAngle`,`outerConeAngle`],u=o||c!==void 0?6:5;return t[2]!==`lights`||t.length!==u||!Number.isInteger(i)||!Array.isArray(a)||!a[i]||!l.includes(s)||o&&s!==`innerConeAngle`&&s!==`outerConeAngle`||c!==void 0&&(!/^[0-2]$/.test(c)||s!==`color`)?(CS(r,`punctual-light pointers must target supported typed light properties`),null):{type:`light`,sampler:n,pointer:r,targetLightIndex:i,property:s,...c===void 0?{}:{component:Number(c)}}}function fS(e,t,n,r){let i=t.length===5&&t[2]===`extensions`&&t[3]===`KHR_node_visibility`&&t[4]===`visible`;if(t.length!==3&&!i)return CS(r,`node pointers must target transforms, morph weights, or KHR_node_visibility.visible`),null;let a=Number(t[1]),o=e.nodes[a];if(!Number.isInteger(a)||!o)return w.warn(`KHR_animation_pointer target ${r} references a missing node and will be skipped`)(),null;if(i&&n.interpolation!==`STEP`)return CS(r,`boolean visibility animation requires STEP interpolation`),null;let s=i?`visibility`:gS(t[2]);return s?{type:`node`,sampler:n,targetNodeId:o.id,path:s}:(CS(r,`node property "${t[2]}" has no runtime animation mapping`),null)}function pS(e,t){let n;if(t.path===`weights`)n=t.node;else if(t.path===`pointer`){let e=t.extensions?.KHR_animation_pointer?.pointer,r=typeof e==`string`?/^\/nodes\/(\d+)\/weights$/.exec(e):null;if(!r)return;n=Number(r[1])}else return;let r=e.nodes[n??0],i=typeof r?.mesh==`number`?e.meshes[r.mesh]:r?.mesh;return r?.weights?.length||i?.weights?.length||i?.primitives?.[0]?.targets?.length||1}function mS(e,t,n,r){let i=n===`CUBICSPLINE`?3:1,a=e.length/(Math.max(t,1)*i),o=r>1?r:Number.isInteger(a)&&a>1?a:r;if(o<=1)return e;let s=e.flat(),c=[];for(let e=0;e<s.length;e+=o)c.push(s.slice(e,e+o));return c}function hS(e,t,n,r){if(t.length<3)return CS(r,`material pointers must include a material index and target property path`),null;let i=Number(t[1]),a=e.materials[i];if(!Number.isInteger(i)||!a)return w.warn(`KHR_animation_pointer target ${r} references a missing material and will be skipped`)(),null;let o=_S(a,t.slice(2));return`reason`in o?(CS(r,o.reason),null):{sampler:n,pointer:r,targetMaterialIndex:i,...o}}function gS(e){switch(e){case`translation`:case`rotation`:case`scale`:case`weights`:return e;default:return null}}function _S(e,t){let n=vS(e,t);if(!(`reason`in n)||n.reason!==`not-a-texture-transform-target`)return n;switch(t.join(`/`)){case`pbrMetallicRoughness/baseColorFactor`:return e.pbrMetallicRoughness?{type:`material`,property:`baseColorFactor`}:{reason:K(t)};case`pbrMetallicRoughness/metallicFactor`:return e.pbrMetallicRoughness?{type:`material`,property:`metallicRoughnessValues`,component:0}:{reason:K(t)};case`pbrMetallicRoughness/roughnessFactor`:return e.pbrMetallicRoughness?{type:`material`,property:`metallicRoughnessValues`,component:1}:{reason:K(t)};case`normalTexture/scale`:return e.normalTexture?{type:`material`,property:`normalScale`}:{reason:K(t)};case`occlusionTexture/strength`:return e.occlusionTexture?{type:`material`,property:`occlusionStrength`}:{reason:K(t)};case`emissiveFactor`:return{type:`material`,property:`emissiveFactor`};case`alphaCutoff`:return{type:`material`,property:`alphaCutoff`};case`extensions/KHR_materials_specular/specularFactor`:return e.extensions?.KHR_materials_specular?{type:`material`,property:`specularIntensityFactor`}:{reason:K(t)};case`extensions/KHR_materials_specular/specularColorFactor`:return e.extensions?.KHR_materials_specular?{type:`material`,property:`specularColorFactor`}:{reason:K(t)};case`extensions/KHR_materials_ior/ior`:return e.extensions?.KHR_materials_ior?{type:`material`,property:`ior`}:{reason:K(t)};case`extensions/EXT_materials_bump/bumpFactor`:return e.extensions?.EXT_materials_bump?{type:`material`,property:`bumpFactor`}:{reason:K(t)};case`extensions/KHR_materials_diffuse_transmission/diffuseTransmissionFactor`:return e.extensions?.KHR_materials_diffuse_transmission?{type:`material`,property:`diffuseTransmissionFactor`}:{reason:K(t)};case`extensions/KHR_materials_diffuse_transmission/diffuseTransmissionColorFactor`:return e.extensions?.KHR_materials_diffuse_transmission?{type:`material`,property:`diffuseTransmissionColorFactor`}:{reason:K(t)};case`extensions/KHR_materials_volume_scatter/multiscatterColorFactor`:case`extensions/KHR_materials_volume_scatter/multiscatterColor`:return e.extensions?.KHR_materials_volume_scatter?{type:`material`,property:`multiscatterColorFactor`}:{reason:K(t)};case`extensions/KHR_materials_volume_scatter/scatterAnisotropy`:return e.extensions?.KHR_materials_volume_scatter?{type:`material`,property:`scatterAnisotropy`}:{reason:K(t)};case`extensions/KHR_materials_dispersion/dispersion`:return e.extensions?.KHR_materials_dispersion?{type:`material`,property:`dispersion`}:{reason:K(t)};case`extensions/KHR_materials_transmission/transmissionFactor`:return e.extensions?.KHR_materials_transmission?{type:`material`,property:`transmissionFactor`}:{reason:K(t)};case`extensions/KHR_materials_volume/thicknessFactor`:return e.extensions?.KHR_materials_volume?{type:`material`,property:`thicknessFactor`}:{reason:K(t)};case`extensions/KHR_materials_volume/attenuationDistance`:return e.extensions?.KHR_materials_volume?{type:`material`,property:`attenuationDistance`}:{reason:K(t)};case`extensions/KHR_materials_volume/attenuationColor`:return e.extensions?.KHR_materials_volume?{type:`material`,property:`attenuationColor`}:{reason:K(t)};case`extensions/KHR_materials_clearcoat/clearcoatFactor`:return e.extensions?.KHR_materials_clearcoat?{type:`material`,property:`clearcoatFactor`}:{reason:K(t)};case`extensions/KHR_materials_clearcoat/clearcoatRoughnessFactor`:return e.extensions?.KHR_materials_clearcoat?{type:`material`,property:`clearcoatRoughnessFactor`}:{reason:K(t)};case`extensions/KHR_materials_sheen/sheenColorFactor`:return e.extensions?.KHR_materials_sheen?{type:`material`,property:`sheenColorFactor`}:{reason:K(t)};case`extensions/KHR_materials_sheen/sheenRoughnessFactor`:return e.extensions?.KHR_materials_sheen?{type:`material`,property:`sheenRoughnessFactor`}:{reason:K(t)};case`extensions/KHR_materials_iridescence/iridescenceFactor`:return e.extensions?.KHR_materials_iridescence?{type:`material`,property:`iridescenceFactor`}:{reason:K(t)};case`extensions/KHR_materials_iridescence/iridescenceIor`:return e.extensions?.KHR_materials_iridescence?{type:`material`,property:`iridescenceIor`}:{reason:K(t)};case`extensions/KHR_materials_iridescence/iridescenceThicknessMinimum`:return e.extensions?.KHR_materials_iridescence?{type:`material`,property:`iridescenceThicknessRange`,component:0}:{reason:K(t)};case`extensions/KHR_materials_iridescence/iridescenceThicknessMaximum`:return e.extensions?.KHR_materials_iridescence?{type:`material`,property:`iridescenceThicknessRange`,component:1}:{reason:K(t)};case`extensions/KHR_materials_anisotropy/anisotropyStrength`:return e.extensions?.KHR_materials_anisotropy?{type:`material`,property:`anisotropyStrength`}:{reason:K(t)};case`extensions/KHR_materials_anisotropy/anisotropyRotation`:return e.extensions?.KHR_materials_anisotropy?{type:`material`,property:`anisotropyRotation`}:{reason:K(t)};case`extensions/KHR_materials_emissive_strength/emissiveStrength`:return e.extensions?.KHR_materials_emissive_strength?{type:`material`,property:`emissiveStrength`}:{reason:K(t)};default:return{reason:K(t)}}}function vS(e,t){let n=t.lastIndexOf(`extensions`);if(n<0||t[n+1]!==`KHR_texture_transform`||n<1)return{reason:`not-a-texture-transform-target`};let r=Nb(t.slice(0,n));if(!r)return{reason:xS(t.slice(0,n))};let i=yS(e,r.pathSegments);if(!i)return{reason:`texture-transform target "${t.slice(0,n).join(`/`)}" does not exist on the referenced material`};let a=t[n+2];if(a===`texCoord`)return{reason:`animated KHR_texture_transform.texCoord is unsupported because texCoord selection is structural, not a runtime float/vector update`};if(a!==`offset`&&a!==`rotation`&&a!==`scale`)return{reason:`KHR_texture_transform property "${a}" is not animatable; supported properties are offset, rotation, and scale`};let o=t[n+3];if(t.length>n+4)return{reason:`KHR_texture_transform.${a} does not support nested property paths`};let s;if(o!==void 0){if(s=Number(o),a===`rotation`)return{reason:`KHR_texture_transform.rotation does not support component indices`};if(!Number.isInteger(s)||s<0||s>1)return{reason:`KHR_texture_transform.${a} component index "${o}" is invalid; only 0 and 1 are supported`}}return{type:`textureTransform`,textureSlot:r.slot,path:a,component:s,baseTransform:jb(i)}}function yS(e,t){let n=e;for(let e of t)if(n=n?.[e],!n)return null;return n}function bS(e){return e.slice(1).split(`/`).map(e=>e.replace(/~1/g,`/`).replace(/~0/g,`~`))}function K(e){let t=SS(e);if(t){let e=rS(t);if(e?.supportLevel===`none`)return`${t} is referenced by this pointer, but ${e.comment.charAt(0).toLowerCase()}${e.comment.slice(1)}`}return`no runtime target exists for material property "${e.join(`/`)}"`}function xS(e){let t=SS(e);if(t){let e=rS(t);if(e?.supportLevel===`none`)return`${t} is referenced by this pointer, but ${e.comment.charAt(0).toLowerCase()}${e.comment.slice(1)}`}return`texture-transform target "${e.join(`/`)}" has no runtime texture-slot mapping`}function SS(e){let t=e.indexOf(`extensions`),n=e[t+1];return t>=0&&n?n:null}function CS(e,t){w.warn(`KHR_animation_pointer target ${e} will be skipped because ${t}`)()}function wS(e,t){if(t.has(e))return t.get(e);let{value:n,components:r}=ES(e);DS(r===1,`accessorToJsArray1D must have exactly 1 component`);let i=Array.from(n);return t.set(e,i),i}function TS(e,t){if(t.has(e))return t.get(e);let{value:n,components:r}=ES(e);DS(r>=1,`accessorToJsArray2D must have at least 1 component`);let i=[];for(let e=0;e<n.length;e+=r)i.push(Array.from(n.slice(e,e+r)));return t.set(e,i),i}function ES(e){if(e.value)return{value:e.value,components:e.components};let t=e.bufferView?.data;DS(t!==void 0),DS(e.componentType===5126);let n=e.type===`SCALAR`?1:Number(e.type.slice(3));return{value:new Float32Array(t.buffer,t.byteOffset+(e.byteOffset||0),e.count*n),components:n}}function DS(e,t){if(!e)throw Error(t)}var OS=class{variants;names;activeVariant=null;modelNodes;constructor(e,t){let n=e.extensions?.KHR_materials_variants?.variants||[];this.variants=n.map((e,t)=>({name:e.name||`Variant-${t}`,index:t})),this.names=this.variants.map(e=>e.name);let r=new Set;for(let e of t)e.preorderTraversal(e=>{e instanceof ca&&e.userData.gltfMaterialVariants&&r.add(e)});this.modelNodes=Array.from(r)}selectVariant(e){let t=this.variants.find(t=>t.name===e);if(!t)throw Error(`Unknown glTF material variant: ${e}`);for(let e of this.modelNodes){let n=e.userData.gltfMaterialVariants,r=n.mappings.get(t.index);e.model.setMaterial(r?.material||n.defaultMaterial),e.model.setParameters(r?.parameters||n.defaultParameters)}this.activeVariant=e}resetVariant(){for(let e of this.modelNodes){let t=e.userData.gltfMaterialVariants;e.model.setMaterial(t.defaultMaterial),e.model.setParameters(t.defaultParameters)}this.activeVariant=null}};function kS(e,t,n){n?.strictExtensions&&eS(t,e);let{scenes:r,materials:i,gltfMeshIdToNodeMap:a,gltfNodeIdToNodeMap:o,gltfNodeIndexToNodeMap:s,generatedTextures:c}=vx(e,t,n),l=sS(t),u=(t.lights||t.extensions?.KHR_lights_punctual?.lights||[]).map(e=>({...e,...Array.isArray(e.color)?{color:[...e.color]}:{},...e.spot?{spot:{...e.spot}}:{}})),d=(t.cameras||[]).map(e=>{let t={...e};return e.perspective&&(t.perspective={...e.perspective}),e.orthographic&&(t.orthographic={...e.orthographic}),t}),f={useByteColors:n?.useByteColors??!0,nodeVisibility:o,lightDefinitions:u},p=Dx(t,f),m=()=>{p.splice(0,p.length,...Dx(t,f))},h=new zx({onVisibilityChange:m,cameras:d,lightDefinitions:u,onLightChange:m,animations:l,gltfNodeIdToNodeMap:o,materials:i}),g=new OS(t,r),_=Qx(t,e),v=r.map(e=>AS(e.getBounds())),y=jS(v),b=new qx({gltf:t,scenes:r,gltfNodeIndexToNodeMap:s});h.setUpdateHandler(()=>b.update());let x=!1;return{scenes:r,materials:i,variants:g,cameras:d,animator:h,animations:l,lights:p,extensionSupport:_,sceneBounds:v,modelBounds:y,gltfMeshIdToNodeMap:a,gltfNodeIdToNodeMap:o,gltfNodeIndexToNodeMap:s,skins:b,gltf:t,destroy:()=>{if(x)return;x=!0;let e=new Set([...r,...a.values(),...o.values()]),t=new Set,n=new Set(i);for(let r of e)r.preorderTraversal(e=>{e instanceof ca&&(t.add(e),e.model?.material&&n.add(e.model.material))});for(let e of t)e.destroy();for(let t of e)t.destroy();for(let e of n)e.destroy();for(let e of c)e.destroy();c.clear()}}}function AS(e){if(!e)return{bounds:null,center:[0,0,0],size:[0,0,0],radius:.5,recommendedOrbitDistance:1};let t=[[e[0][0],e[0][1],e[0][2]],[e[1][0],e[1][1],e[1][2]]],n=[t[1][0]-t[0][0],t[1][1]-t[0][1],t[1][2]-t[0][2]],r=[t[0][0]+n[0]*.5,t[0][1]+n[1]*.5,t[0][2]+n[2]*.5],i=Math.max(n[0],n[1],n[2])*.5,a=Math.max(.5*Math.hypot(n[0],n[1],n[2]),.001);return{bounds:t,center:r,size:n,radius:a,recommendedOrbitDistance:Math.max(Math.max(i,.001)/Math.tan(Math.PI/6)*1.15,a*1.1)}}function jS(e){let t=null;for(let n of e)if(n.bounds){if(!t){t=[[...n.bounds[0]],[...n.bounds[1]]];continue}for(let e=0;e<3;e++)t[0][e]=Math.min(t[0][e],n.bounds[0][e]),t[1][e]=Math.max(t[1][e],n.bounds[1][e])}return AS(t)}var MS=`4.5.4`;async function NS(e,t,n,r){return r._parse(e,t,n,r)}function PS(e,t){if(!e)throw Error(t||`loader assertion failed.`)}var FS={self:typeof self<`u`&&self,window:typeof window<`u`&&window,global:typeof global<`u`&&global,document:typeof document<`u`&&document};FS.self||FS.window||FS.global,FS.window||FS.self||FS.global,FS.global||FS.self||FS.window,FS.document;var IS=!!(typeof process!=`object`||String(process)!==`[object process]`||process.browser),LS=typeof process<`u`&&process.version&&/v([0-9]*)/.exec(process.version);LS&&parseFloat(LS[1]);var RS=e=>typeof SharedArrayBuffer<`u`&&e instanceof SharedArrayBuffer;function zS(e,t=5){return typeof e==`string`?e.slice(0,t):ArrayBuffer.isView(e)?BS(e.buffer,e.byteOffset,t):e instanceof ArrayBuffer?BS(e,0,t):``}function BS(e,t,n){if(e.byteLength<=t+n)return``;let r=new DataView(e),i=``;for(let e=0;e<n;e++)i+=String.fromCharCode(r.getUint8(t+e));return i}function VS(e){try{return JSON.parse(e)}catch{throw Error(`Failed to parse JSON from data starting with "${zS(e)}"`)}}function HS(e,t,n){let r=n===void 0?new Uint8Array(e).subarray(t):new Uint8Array(e).subarray(t,t+n);return new Uint8Array(r).buffer}function US(e,t){return PS(e>=0),PS(t>0),e+(t-1)&~(t-1)}function WS(e,t,n){let r;if(e instanceof ArrayBuffer)r=new Uint8Array(e);else{let t=e.byteOffset,n=e.byteLength;r=new Uint8Array(e.buffer||e.arrayBuffer,t,n)}return t.set(r,n),n+US(r.byteLength,4)}function GS(e){if(e instanceof ArrayBuffer)return e;if(RS(e))return KS(e);let{buffer:t,byteOffset:n,byteLength:r}=e;return t instanceof ArrayBuffer&&n===0&&r===t.byteLength?t:KS(t,n,r)}function KS(e,t=0,n=e.byteLength-t){let r=new Uint8Array(e,t,n),i=new Uint8Array(r.length);return i.set(r),i.buffer}var qS=globalThis.loaders?.parseImageNode,JS=typeof Image<`u`,YS=typeof ImageBitmap<`u`,XS=IS?!0:!!qS;function ZS(e){switch(e){case`auto`:return YS||JS||XS;case`imagebitmap`:return YS;case`image`:return JS;case`data`:return XS;default:throw Error(`@loaders.gl/images: image ${e} not supported in this environment`)}}function QS(){if(YS)return`imagebitmap`;if(JS)return`image`;if(XS)return`data`;throw Error(`Install '@loaders.gl/polyfills' to parse images under Node.js`)}function $S(e){let t=tC(e);if(!t)throw Error(`Not an image`);return t}function eC(e){switch($S(e)){case`data`:return e;case`image`:case`imagebitmap`:let t=document.createElement(`canvas`),n=t.getContext(`2d`);if(!n)throw Error(`getImageData`);return t.width=e.width,t.height=e.height,n.drawImage(e,0,0),n.getImageData(0,0,e.width,e.height);default:throw Error(`getImageData`)}}function tC(e){return typeof ImageBitmap<`u`&&e instanceof ImageBitmap?`imagebitmap`:typeof Image<`u`&&e instanceof Image?`image`:e&&typeof e==`object`&&e.data&&e.width&&e.height?`data`:null}var nC=/^data:image\/svg\+xml/,rC=/\.svg((\?|#).*)?$/;function iC(e){return e&&(nC.test(e)||rC.test(e))}function aC(e,t){if(iC(t)){let t=new TextDecoder().decode(e);try{typeof unescape==`function`&&typeof encodeURIComponent==`function`&&(t=unescape(encodeURIComponent(t)))}catch(e){throw Error(e.message)}return`data:image/svg+xml;base64,${btoa(t)}`}return oC(e,t)}function oC(e,t){if(iC(t))throw Error(`SVG cannot be parsed directly to imagebitmap`);return new Blob([new Uint8Array(e)])}async function sC(e,t,n){let r=aC(e,n),i=self.URL||self.webkitURL,a=typeof r!=`string`&&i.createObjectURL(r);try{return await cC(a||r,t)}finally{a&&i.revokeObjectURL(a)}}async function cC(e,t){let n=new Image;return n.src=e,t.image&&t.image.decode&&n.decode?(await n.decode(),n):await new Promise((e,t)=>{try{n.onload=()=>e(n),n.onerror=e=>{let n=e instanceof Error?e.message:`error`;t(Error(n))}}catch(e){t(e)}})}var lC=!0;async function uC(e,t,n){let r;r=iC(n)?await sC(e,t,n):oC(e,n);let i=t&&t.imagebitmap;return await dC(r,i)}async function dC(e,t=null){if((fC(t)||!lC)&&(t=null),t)try{return await createImageBitmap(e,t)}catch(e){console.warn(e),lC=!1}return await createImageBitmap(e)}function fC(e){if(!e)return!0;for(let t in e)if(Object.prototype.hasOwnProperty.call(e,t))return!1;return!0}function pC(e){return!_C(e,`ftyp`,4)||!(e[8]&96)?null:mC(e)}function mC(e){switch(hC(e,8,12).replace(`\0`,` `).trim()){case`avif`:case`avis`:return{extension:`avif`,mimeType:`image/avif`};default:return null}}function hC(e,t,n){return String.fromCharCode(...e.slice(t,n))}function gC(e){return[...e].map(e=>e.charCodeAt(0))}function _C(e,t,n=0){let r=gC(t);for(let t=0;t<r.length;++t)if(r[t]!==e[t+n])return!1;return!0}var vC=!1,yC=!0;function bC(e){let t=DC(e);return SC(t)||TC(t)||CC(t)||wC(t)||xC(t)}function xC(e){let t=pC(new Uint8Array(e instanceof DataView?e.buffer:e));return t?{mimeType:t.mimeType,width:0,height:0}:null}function SC(e){let t=DC(e);return t.byteLength>=24&&t.getUint32(0,vC)===2303741511?{mimeType:`image/png`,width:t.getUint32(16,vC),height:t.getUint32(20,vC)}:null}function CC(e){let t=DC(e);return t.byteLength>=10&&t.getUint32(0,vC)===1195984440?{mimeType:`image/gif`,width:t.getUint16(6,yC),height:t.getUint16(8,yC)}:null}function wC(e){let t=DC(e);return t.byteLength>=14&&t.getUint16(0,vC)===16973&&t.getUint32(2,yC)===t.byteLength?{mimeType:`image/bmp`,width:t.getUint32(18,yC),height:t.getUint32(22,yC)}:null}function TC(e){let t=DC(e);if(!(t.byteLength>=3&&t.getUint16(0,vC)===65496&&t.getUint8(2)===255))return null;let{tableMarkers:n,sofMarkers:r}=EC(),i=2;for(;i+9<t.byteLength;){let e=t.getUint16(i,vC);if(r.has(e))return{mimeType:`image/jpeg`,height:t.getUint16(i+5,vC),width:t.getUint16(i+7,vC)};if(!n.has(e))return null;i+=2,i+=t.getUint16(i,vC)}return null}function EC(){let e=new Set([65499,65476,65484,65501,65534]);for(let t=65504;t<65520;++t)e.add(t);return{tableMarkers:e,sofMarkers:new Set([65472,65473,65474,65475,65477,65478,65479,65481,65482,65483,65485,65486,65487,65502])}}function DC(e){if(e instanceof DataView)return e;if(ArrayBuffer.isView(e))return new DataView(e.buffer);if(e instanceof ArrayBuffer)return new DataView(e);throw Error(`toDataView`)}async function OC(e,t){let{mimeType:n}=bC(e)||{},r=globalThis.loaders?.parseImageNode;return PS(r),await r(e,n)}async function kC(e,t,n){t||={};let r=(t.image||{}).type||`auto`,{url:i}=n||{},a=AC(r),o;switch(a){case`imagebitmap`:o=await uC(e,t,i);break;case`image`:o=await sC(e,t,i);break;case`data`:o=await OC(e,t);break;default:PS(!1)}return r===`data`&&(o=eC(o)),o}function AC(e){switch(e){case`auto`:case`data`:return QS();default:return ZS(e),e}}var jC={dataType:null,batchType:null,id:`image`,module:`images`,name:`Images`,version:MS,mimeTypes:[`image/png`,`image/jpeg`,`image/gif`,`image/webp`,`image/avif`,`image/bmp`,`image/vnd.microsoft.icon`,`image/svg+xml`],extensions:[`png`,`jpg`,`jpeg`,`gif`,`webp`,`bmp`,`ico`,`svg`,`avif`],parse:kC,tests:[e=>!!bC(new DataView(e))],options:{image:{type:`auto`,decode:!0}}},MC=[`image/png`,`image/jpeg`,`image/gif`,`image/webp`,`image/avif`,`image/tiff`,`image/svg`,`image/svg+xml`,`image/bmp`,`image/vnd.microsoft.icon`],NC=null;async function PC(){return NC||=FC(),await NC}async function FC(){let e=new Set;for(let t of MC)(IS?await VC(t):RC(t))&&e.add(t);return e}var IC={};function LC(e){return IC[e]===void 0&&(IC[e]=IS?zC(e):RC(e)),IC[e]}function RC(e){let t=globalThis.loaders?.imageFormatsNode||[`image/png`,`image/jpeg`,`image/gif`];return!!globalThis.loaders?.parseImageNode&&t.includes(e)}function zC(e){switch(e){case`image/avif`:case`image/webp`:return HC(e);default:return!0}}var BC={"image/avif":`data:image/avif;base64,AAAAIGZ0eXBhdmlmAAAAAGF2aWZtaWYxbWlhZk1BMUIAAADybWV0YQAAAAAAAAAoaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAGxpYmF2aWYAAAAADnBpdG0AAAAAAAEAAAAeaWxvYwAAAABEAAABAAEAAAABAAABGgAAAB0AAAAoaWluZgAAAAAAAQAAABppbmZlAgAAAAABAABhdjAxQ29sb3IAAAAAamlwcnAAAABLaXBjbwAAABRpc3BlAAAAAAAAAAIAAAACAAAAEHBpeGkAAAAAAwgICAAAAAxhdjFDgQ0MAAAAABNjb2xybmNseAACAAIAAYAAAAAXaXBtYQAAAAAAAAABAAEEAQKDBAAAACVtZGF0EgAKCBgANogQEAwgMg8f8D///8WfhwB8+ErK42A=`,"image/webp":`data:image/webp;base64,UklGRiIAAABXRUJQVlA4IBYAAAAwAQCdASoBAAEADsD+JaQAA3AAAAAA`};async function VC(e){let t=BC[e];return!t||await UC(t)}function HC(e){try{return document.createElement(`canvas`).toDataURL(e).indexOf(`data:${e}`)===0}catch{return!1}}async function UC(e){return new Promise(t=>{let n=new Image;n.src=e,n.onload=()=>t(n.height>0),n.onerror=()=>t(!1)})}function WC(e,t){if(!e)throw Error(t||`assert failed: gltf`)}var GC={SCALAR:1,VEC2:2,VEC3:3,VEC4:4,MAT2:4,MAT3:9,MAT4:16},KC={5120:1,5121:1,5122:2,5123:2,5125:4,5126:4},qC=1.33,JC=[`SCALAR`,`VEC2`,`VEC3`,`VEC4`],YC=new Map([[Int8Array,5120],[Uint8Array,5121],[Int16Array,5122],[Uint16Array,5123],[Uint32Array,5125],[Float32Array,5126],[Float64Array,5130]]),XC={SCALAR:1,VEC2:2,VEC3:3,VEC4:4,MAT2:4,MAT3:9,MAT4:16},ZC={5120:1,5121:1,5122:2,5123:2,5125:4,5126:4},QC={5120:Int8Array,5121:Uint8Array,5122:Int16Array,5123:Uint16Array,5125:Uint32Array,5126:Float32Array};function $C(e){return JC[e-1]||JC[0]}function ew(e){let t=YC.get(e.constructor);if(!t)throw Error(`Illegal typed array`);return t}function tw(e,t){let n=QC[e.componentType],r=XC[e.type],i=ZC[e.componentType],a=e.count*r,o=e.count*r*i;return WC(o>=0&&o<=t.byteLength),{ArrayType:n,length:a,byteLength:o,componentByteSize:KC[e.componentType],numberOfComponentsInElement:GC[e.type]}}function nw(e){let{images:t,bufferViews:n}=e;t||=[],n||=[];let r=t.map(e=>e.bufferView);n=n.filter(e=>!r.includes(e));let i=n.reduce((e,t)=>e+t.byteLength,0),a=t.reduce((e,t)=>{let{width:n,height:r}=t.image;return e+n*r},0);return i+Math.ceil(4*a*qC)}function rw(e,t,n){let r=e.bufferViews[n];WC(r);let i=t[r.buffer];WC(i);let a=(r.byteOffset||0)+i.byteOffset;return new Uint8Array(i.arrayBuffer,a,r.byteLength)}function iw(e,t,n){let r=typeof n==`number`?e.accessors?.[n]:n;if(!r)throw Error(`No gltf accessor ${JSON.stringify(n)}`);let i=e.bufferViews?.[r.bufferView||0];if(!i)throw Error(`No gltf buffer view for accessor ${i}`);let{arrayBuffer:a,byteOffset:o}=t[i.buffer],s=(o||0)+(r.byteOffset||0)+(i.byteOffset||0),{ArrayType:c,length:l,componentByteSize:u,numberOfComponentsInElement:d}=tw(r,i),f=u*d,p=i.byteStride||f;if(i.byteStride===void 0||i.byteStride===f)return new c(a,s,l);let m=new c(l);for(let e=0;e<r.count;e++){let t=new c(a,s+e*p,d);m.set(t,e*d)}return m}function aw(){return{asset:{version:`2.0`,generator:`loaders.gl`},buffers:[],extensions:{},extensionsRequired:[],extensionsUsed:[]}}var q=class{gltf;sourceBuffers;byteLength;constructor(e){this.gltf={json:e?.json||aw(),buffers:e?.buffers||[],images:e?.images||[]},this.sourceBuffers=[],this.byteLength=0,this.gltf.buffers&&this.gltf.buffers[0]&&(this.byteLength=this.gltf.buffers[0].byteLength,this.sourceBuffers=[this.gltf.buffers[0]])}get json(){return this.gltf.json}getApplicationData(e){return this.json[e]}getExtraData(e){return(this.json.extras||{})[e]}hasExtension(e){let t=this.getUsedExtensions().find(t=>t===e),n=this.getRequiredExtensions().find(t=>t===e);return typeof t==`string`||typeof n==`string`}getExtension(e){let t=this.getUsedExtensions().find(t=>t===e),n=this.json.extensions||{};return t?n[e]:null}getRequiredExtension(e){return this.getRequiredExtensions().find(t=>t===e)?this.getExtension(e):null}getRequiredExtensions(){return this.json.extensionsRequired||[]}getUsedExtensions(){return this.json.extensionsUsed||[]}getRemovedExtensions(){return this.json.extensionsRemoved||[]}getObjectExtension(e,t){return(e.extensions||{})[t]}getScene(e){return this.getObject(`scenes`,e)}getNode(e){return this.getObject(`nodes`,e)}getSkin(e){return this.getObject(`skins`,e)}getMesh(e){return this.getObject(`meshes`,e)}getMaterial(e){return this.getObject(`materials`,e)}getAccessor(e){return this.getObject(`accessors`,e)}getTexture(e){return this.getObject(`textures`,e)}getSampler(e){return this.getObject(`samplers`,e)}getImage(e){return this.getObject(`images`,e)}getBufferView(e){return this.getObject(`bufferViews`,e)}getBuffer(e){return this.getObject(`buffers`,e)}getObject(e,t){if(typeof t==`object`)return t;let n=this.json[e]&&this.json[e][t];if(!n)throw Error(`glTF file error: Could not find ${e}[${t}]`);return n}getTypedArrayForBufferView(e){e=this.getBufferView(e);let t=e.buffer,n=this.gltf.buffers[t];WC(n);let r=(e.byteOffset||0)+n.byteOffset;return new Uint8Array(n.arrayBuffer,r,e.byteLength)}getTypedArrayForAccessor(e){let t=this.getAccessor(e);return iw(this.gltf.json,this.gltf.buffers,t)}getTypedArrayForImageData(e){e=this.getAccessor(e);let t=this.getBufferView(e.bufferView),n=this.getBuffer(t.buffer).data,r=t.byteOffset||0;return new Uint8Array(n,r,t.byteLength)}addApplicationData(e,t){return this.json[e]=t,this}addExtraData(e,t){return this.json.extras=this.json.extras||{},this.json.extras[e]=t,this}addObjectExtension(e,t,n){return e.extensions=e.extensions||{},e.extensions[t]=n,this.registerUsedExtension(t),this}setObjectExtension(e,t,n){let r=e.extensions||{};r[t]=n}removeObjectExtension(e,t){let n=e?.extensions||{};if(n[t]){this.json.extensionsRemoved=this.json.extensionsRemoved||[];let e=this.json.extensionsRemoved;e.includes(t)||e.push(t)}delete n[t]}addExtension(e,t={}){return WC(t),this.json.extensions=this.json.extensions||{},this.json.extensions[e]=t,this.registerUsedExtension(e),t}addRequiredExtension(e,t={}){return WC(t),this.addExtension(e,t),this.registerRequiredExtension(e),t}registerUsedExtension(e){this.json.extensionsUsed=this.json.extensionsUsed||[],this.json.extensionsUsed.find(t=>t===e)||this.json.extensionsUsed.push(e)}registerRequiredExtension(e){this.registerUsedExtension(e),this.json.extensionsRequired=this.json.extensionsRequired||[],this.json.extensionsRequired.find(t=>t===e)||this.json.extensionsRequired.push(e)}removeExtension(e){if(this.json.extensions?.[e]){this.json.extensionsRemoved=this.json.extensionsRemoved||[];let t=this.json.extensionsRemoved;t.includes(e)||t.push(e)}this.json.extensions&&delete this.json.extensions[e],this.json.extensionsRequired&&this._removeStringFromArray(this.json.extensionsRequired,e),this.json.extensionsUsed&&this._removeStringFromArray(this.json.extensionsUsed,e)}setDefaultScene(e){this.json.scene=e}addScene(e){let{nodeIndices:t}=e;return this.json.scenes=this.json.scenes||[],this.json.scenes.push({nodes:t}),this.json.scenes.length-1}addNode(e){let{meshIndex:t,matrix:n}=e;this.json.nodes=this.json.nodes||[];let r={mesh:t};return n&&(r.matrix=n),this.json.nodes.push(r),this.json.nodes.length-1}addMesh(e){let{attributes:t,indices:n,material:r,mode:i=4}=e,a={primitives:[{attributes:this._addAttributes(t),mode:i}]};if(n){let e=this._addIndices(n);a.primitives[0].indices=e}return Number.isFinite(r)&&(a.primitives[0].material=r),this.json.meshes=this.json.meshes||[],this.json.meshes.push(a),this.json.meshes.length-1}addPointCloud(e){let t={primitives:[{attributes:this._addAttributes(e),mode:0}]};return this.json.meshes=this.json.meshes||[],this.json.meshes.push(t),this.json.meshes.length-1}addImage(e,t){let n=bC(e),r=t||n?.mimeType,i={bufferView:this.addBufferView(e),mimeType:r};return this.json.images=this.json.images||[],this.json.images.push(i),this.json.images.length-1}addBufferView(e,t=0,n=this.byteLength){let r=e.byteLength;WC(Number.isFinite(r)),this.sourceBuffers=this.sourceBuffers||[],this.sourceBuffers.push(e);let i={buffer:t,byteOffset:n,byteLength:r};return this.byteLength+=US(r,4),this.json.bufferViews=this.json.bufferViews||[],this.json.bufferViews.push(i),this.json.bufferViews.length-1}addAccessor(e,t){let n={bufferView:e,type:$C(t.size),componentType:t.componentType,count:t.count,max:t.max,min:t.min};return this.json.accessors=this.json.accessors||[],this.json.accessors.push(n),this.json.accessors.length-1}addBinaryBuffer(e,t={size:3}){let n=this.addBufferView(e),r={min:t.min,max:t.max};(!r.min||!r.max)&&(r=this._getAccessorMinMax(e,t.size));let i={size:t.size,componentType:ew(e),count:Math.round(e.length/t.size),min:r.min,max:r.max};return this.addAccessor(n,Object.assign(i,t))}addTexture(e){let{imageIndex:t}=e,n={source:t};return this.json.textures=this.json.textures||[],this.json.textures.push(n),this.json.textures.length-1}addMaterial(e){return this.json.materials=this.json.materials||[],this.json.materials.push(e),this.json.materials.length-1}createBinaryChunk(){let e=this.byteLength,t=new ArrayBuffer(e),n=new Uint8Array(t),r=0;for(let e of this.sourceBuffers||[])r=WS(e,n,r);this.json?.buffers?.[0]?this.json.buffers[0].byteLength=e:this.json.buffers=[{byteLength:e}],this.gltf.binary=t,this.sourceBuffers=[t],this.gltf.buffers=[{arrayBuffer:t,byteOffset:0,byteLength:t.byteLength}]}_removeStringFromArray(e,t){let n=!0;for(;n;){let r=e.indexOf(t);r>-1?e.splice(r,1):n=!1}}_addAttributes(e={}){let t={};for(let n in e){let r=e[n],i=this._getGltfAttributeName(n);t[i]=this.addBinaryBuffer(r.value,r)}return t}_addIndices(e){return this.addBinaryBuffer(e,{size:1})}_getGltfAttributeName(e){switch(e.toLowerCase()){case`position`:case`positions`:case`vertices`:return`POSITION`;case`normal`:case`normals`:return`NORMAL`;case`color`:case`colors`:return`COLOR_0`;case`texcoord`:case`texcoords`:return`TEXCOORD_0`;default:return e}}_getAccessorMinMax(e,t){let n={min:null,max:null};if(e.length<t)return n;n.min=[],n.max=[];let r=e.subarray(0,t);for(let e of r)n.min.push(e),n.max.push(e);for(let r=t;r<e.length;r+=t)for(let i=0;i<t;i++)n.min[0+i]=Math.min(n.min[0+i],e[r+i]),n.max[0+i]=Math.max(n.max[0+i],e[r+i]);return n}};function ow(e){return(e%1+1)%1}var sw={SCALAR:1,VEC2:2,VEC3:3,VEC4:4,MAT2:4,MAT3:9,MAT4:16,BOOLEAN:1,STRING:1,ENUM:1},cw={INT8:Int8Array,UINT8:Uint8Array,INT16:Int16Array,UINT16:Uint16Array,INT32:Int32Array,UINT32:Uint32Array,INT64:BigInt64Array,UINT64:BigUint64Array,FLOAT32:Float32Array,FLOAT64:Float64Array},lw={INT8:1,UINT8:1,INT16:2,UINT16:2,INT32:4,UINT32:4,INT64:8,UINT64:8,FLOAT32:4,FLOAT64:8};function uw(e,t){return lw[t]*sw[e]}function dw(e,t,n,r){if(n!==`UINT8`&&n!==`UINT16`&&n!==`UINT32`&&n!==`UINT64`)return null;let i=fw(e.getTypedArrayForBufferView(t),`SCALAR`,n,r+1);return i instanceof BigInt64Array||i instanceof BigUint64Array?null:i}function fw(e,t,n,r=1){let i=sw[t],a=cw[n],o=lw[n],s=r*i,c=s*o,l=e.buffer,u=e.byteOffset;return u%o!==0&&(l=new Uint8Array(l).slice(u,u+c).buffer,u=0),new a(GS(l),u,s)}function pw(e,t,n){let r=`TEXCOORD_${t.texCoord||0}`,i=n.attributes[r],a=e.getTypedArrayForAccessor(i),o=e.gltf.json,s=t.index,c=o.textures?.[s]?.source;if(c!==void 0){let n=o.images?.[c]?.mimeType,r=e.gltf.images?.[c];if(r&&r.width!==void 0){let e=[];for(let i=0;i<a.length;i+=2){let o=hw(r,n,a,i,t.channels);e.push(o)}return e}}return[]}function mw(e,t,n,r,i){if(!n?.length)return;let a=[];for(let e of n){let t=r.findIndex(t=>t===e);t===-1&&(t=r.push(e)-1),a.push(t)}let o=new Uint32Array(a),s=e.gltf.buffers.push({arrayBuffer:o.buffer,byteOffset:o.byteOffset,byteLength:o.byteLength})-1,c=e.addBufferView(o,s,0),l=e.addAccessor(c,{size:1,componentType:ew(o),count:o.length});i.attributes[t]=l}function hw(e,t,n,r,i=[0]){let a={r:{offset:0,shift:0},g:{offset:1,shift:8},b:{offset:2,shift:16},a:{offset:3,shift:24}},o=n[r],s=n[r+1],c=1;t&&(t.indexOf(`image/jpeg`)!==-1||t.indexOf(`image/png`)!==-1)&&(c=4);let l=gw(o,s,e,c),u=0;for(let t of i){let n=typeof t==`number`?Object.values(a)[t]:a[t],r=l+n.offset,i=eC(e);if(i.data.length<=r)throw Error(`${i.data.length} <= ${r}`);let o=i.data[r];u|=o<<n.shift}return u}function gw(e,t,n,r=1){let i=n.width,a=ow(e)*(i-1),o=Math.round(a),s=n.height,c=ow(t)*(s-1),l=Math.round(c),u=n.components?n.components:r;return(l*i+o)*u}function _w(e,t,n,r,i){let a=[];for(let o=0;o<t;o++){let t=n[o],s=n[o+1]-n[o];if(s+t>r)break;let c=t/i,l=s/i;a.push(e.slice(c,c+l))}return a}function vw(e,t,n){let r=[];for(let i=0;i<t;i++){let t=i*n;r.push(e.slice(t,t+n))}return r}function yw(e,t,n,r){if(n)throw Error(`Not implemented - arrayOffsets for strings is specified`);if(r){let n=[],i=new TextDecoder(`utf8`),a=0;for(let o=0;o<e;o++){let e=r[o+1]-r[o];if(e+a<=t.length){let r=t.subarray(a,e+a),o=i.decode(r);n.push(o),a+=e}}return n}return[]}var bw=e({createExtMeshFeatures:()=>Ow,decode:()=>Cw,encode:()=>ww,name:()=>Sw}),xw=`EXT_mesh_features`,Sw=xw;async function Cw(e,t){Tw(new q(e),t)}function ww(e,t){let n=new q(e);return Dw(n,t),n.createBinaryChunk(),n.gltf}function Tw(e,t){let n=e.gltf.json;if(n.meshes)for(let r of n.meshes)for(let n of r.primitives)Ew(e,n,t)}function Ew(e,t,n){if(!n?.gltf?.loadBuffers)return;let r=t.extensions?.[xw]?.featureIds;if(r)for(let i of r){let r;if(i.attribute!==void 0){let n=`_FEATURE_ID_${i.attribute}`,a=t.attributes[n];r=e.getTypedArrayForAccessor(a)}else r=i.texture!==void 0&&n?.gltf?.loadImages?pw(e,i.texture,t):[];i.data=r}}function Dw(e,t){let n=e.gltf.json.meshes;if(n)for(let t of n)for(let n of t.primitives)kw(e,n)}function Ow(e,t,n,r){t.extensions||={};let i=t.extensions[xw];i||(i={featureIds:[]},t.extensions[xw]=i);let{featureIds:a}=i,o={featureCount:n.length,propertyTable:r,data:n};a.push(o),e.addObjectExtension(t,xw,i)}function kw(e,t){let n=t.extensions?.[xw];if(!n)return;let r=n.featureIds;r.forEach((n,i)=>{if(n.data){let{accessorKey:a,index:o}=Aw(t.attributes),s=new Uint32Array(n.data);r[i]={featureCount:s.length,propertyTable:n.propertyTable,attribute:o},e.gltf.buffers.push({arrayBuffer:s.buffer,byteOffset:s.byteOffset,byteLength:s.byteLength});let c=e.addBufferView(s),l=e.addAccessor(c,{size:1,componentType:ew(s),count:s.length});t.attributes[a]=l}})}function Aw(e){let t=`_FEATURE_ID_`,n=Object.keys(e).filter(e=>e.indexOf(t)===0),r=-1;for(let e of n){let t=Number(e.substring(12));t>r&&(r=t)}return r++,{accessorKey:`${t}${r}`,index:r}}var jw=e({createExtStructuralMetadata:()=>tT,decode:()=>Pw,encode:()=>Fw,name:()=>Nw}),Mw=`EXT_structural_metadata`,Nw=Mw;async function Pw(e,t){Iw(new q(e),t)}function Fw(e,t){let n=new q(e);return $w(n,t),n.createBinaryChunk(),n.gltf}function Iw(e,t){if(!t.gltf?.loadBuffers)return;let n=e.getExtension(Mw);n&&(t.gltf?.loadImages&&Lw(e,n),Rw(e,n))}function Lw(e,t){let n=t.propertyTextures,r=e.gltf.json;if(n&&r.meshes)for(let i of r.meshes)for(let r of i.primitives)Bw(e,n,r,t)}function Rw(e,t){let n=t.schema;if(!n)return;let r=n.classes,i=t.propertyTables;if(r&&i)for(let t in r){let r=zw(i,t);r&&Hw(e,n,r)}}function zw(e,t){for(let n of e)if(n.class===t)return n;return null}function Bw(e,t,n,r){if(!t)return;let i=n.extensions?.[Mw]?.propertyTextures;if(i)for(let a of i){let i=t[a];Vw(e,i,n,r)}}function Vw(e,t,n,r){if(!t.properties)return;r.dataAttributeNames||=[];let i=t.class;for(let a in t.properties){let o=`${i}_${a}`,s=t.properties?.[a];if(!s)continue;s.data||=[];let c=s.data,l=pw(e,s,n);l!==null&&(mw(e,o,l,c,n),s.data=c,r.dataAttributeNames.push(o))}}function Hw(e,t,n){let r=t.classes?.[n.class];if(!r)throw Error(`Incorrect data in the EXT_structural_metadata extension: no schema class with name ${n.class}`);let i=n.count;for(let a in r.properties){let o=r.properties[a],s=n.properties?.[a];s&&(s.data=Uw(e,t,o,i,s))}}function Uw(e,t,n,r,i){let a=[],o=i.values,s=e.getTypedArrayForBufferView(o),c=Ww(e,n,i,r),l=Gw(e,i,r);switch(n.type){case`SCALAR`:case`VEC2`:case`VEC3`:case`VEC4`:case`MAT2`:case`MAT3`:case`MAT4`:a=Kw(n,r,s,c);break;case`BOOLEAN`:throw Error(`Not implemented - classProperty.type=${n.type}`);case`STRING`:a=yw(r,s,c,l);break;case`ENUM`:a=qw(t,n,r,s,c);break;default:throw Error(`Unknown classProperty type ${n.type}`)}return a}function Ww(e,t,n,r){return t.array&&t.count===void 0&&n.arrayOffsets!==void 0?dw(e,n.arrayOffsets,n.arrayOffsetType||`UINT32`,r):null}function Gw(e,t,n){return t.stringOffsets===void 0?null:dw(e,t.stringOffsets,t.stringOffsetType||`UINT32`,n)}function Kw(e,t,n,r){let i=e.array,a=e.count,o=uw(e.type,e.componentType),s=n.byteLength/o,c;return c=e.componentType?fw(n,e.type,e.componentType,s):n,i?r?_w(c,t,r,n.length,o):a?vw(c,t,a):[]:c}function qw(e,t,n,r,i){let a=t.enumType;if(!a)throw Error(`Incorrect data in the EXT_structural_metadata extension: classProperty.enumType is not set for type ENUM`);let o=e.enums?.[a];if(!o)throw Error(`Incorrect data in the EXT_structural_metadata extension: schema.enums does't contain ${a}`);let s=o.valueType||`UINT16`,c=uw(t.type,s),l=r.byteLength/c,u=fw(r,t.type,s,l);if(u||=r,t.array){if(i)return Jw({valuesData:u,numberOfElements:n,arrayOffsets:i,valuesDataBytesLength:r.length,elementSize:c,enumEntry:o});let e=t.count;return e?Yw(u,n,e,o):[]}return Xw(u,0,n,o)}function Jw(e){let{valuesData:t,numberOfElements:n,arrayOffsets:r,valuesDataBytesLength:i,elementSize:a,enumEntry:o}=e,s=[];for(let e=0;e<n;e++){let n=r[e],c=r[e+1]-r[e];if(c+n>i)break;let l=Xw(t,n/a,c/a,o);s.push(l)}return s}function Yw(e,t,n,r){let i=[];for(let a=0;a<t;a++){let t=Xw(e,n*a,n,r);i.push(t)}return i}function Xw(e,t,n,r){let i=[];for(let a=0;a<n;a++)if(e instanceof BigInt64Array||e instanceof BigUint64Array)i.push(``);else{let n=e[t+a],o=Zw(r,n);o?i.push(o.name):i.push(``)}return i}function Zw(e,t){for(let n of e.values)if(n.value===t)return n;return null}var Qw=`schemaClassId`;function $w(e,t){let n=e.getExtension(Mw);if(n&&n.propertyTables)for(let t of n.propertyTables){let r=t.class,i=n.schema?.classes?.[r];t.properties&&i&&eT(t,i,e)}}function eT(e,t,n){for(let r in e.properties){let i=e.properties[r].data;if(i){let a=t.properties[r];if(a){let t=iT(i,a,n);e.properties[r]=t}}}}function tT(e,t,n=Qw){let r=e.getExtension(Mw);r||=e.addExtension(Mw),r.schema=nT(t,n,r.schema);let i=rT(t,n,r.schema);return r.propertyTables||=[],r.propertyTables.push(i)-1}function nT(e,t,n){let r=n??{id:`schema_id`},i={properties:{}};for(let t of e){let e={type:t.elementType,componentType:t.componentType};i.properties[t.name]=e}return r.classes={},r.classes[t]=i,r}function rT(e,t,n){let r={class:t,count:0},i=0,a=n.classes?.[t];for(let t of e){if(i===0&&(i=t.values.length),i!==t.values.length&&t.values.length)throw Error(`Illegal values in attributes`);a?.properties[t.name]&&(r.properties||={},r.properties[t.name]={values:0,data:t.values})}return r.count=i,r}function iT(e,t,n){let r={values:0};if(t.type===`STRING`){let{stringData:t,stringOffsets:i}=sT(e);r.stringOffsets=cT(i,n),r.values=cT(t,n)}else t.type===`SCALAR`&&t.componentType&&(r.values=cT(oT(e,t.componentType),n));return r}var aT={INT8:Int8Array,UINT8:Uint8Array,INT16:Int16Array,UINT16:Uint16Array,INT32:Int32Array,UINT32:Uint32Array,INT64:Int32Array,UINT64:Uint32Array,FLOAT32:Float32Array,FLOAT64:Float64Array};function oT(e,t){let n=[];for(let t of e)n.push(Number(t));let r=aT[t];if(!r)throw Error(`Illegal component type`);return new r(n)}function sT(e){let t=new TextEncoder,n=[],r=0;for(let i of e){let e=t.encode(i);r+=e.length,n.push(e)}let i=new Uint8Array(r),a=[],o=0;for(let e of n)i.set(e,o),a.push(o),o+=e.length;return a.push(o),{stringData:i,stringOffsets:new Uint32Array(a)}}function cT(e,t){return t.gltf.buffers.push({arrayBuffer:GS(e.buffer),byteOffset:e.byteOffset,byteLength:e.byteLength}),t.addBufferView(e)}var lT=e({decode:()=>fT,name:()=>dT}),uT=`EXT_feature_metadata`,dT=uT;async function fT(e,t){pT(new q(e),t)}function pT(e,t){if(!t.gltf?.loadBuffers)return;let n=e.getExtension(uT);n&&(t.gltf?.loadImages&&mT(e,n),hT(e,n))}function mT(e,t){let n=t.schema;if(!n)return;let r=n.classes,{featureTextures:i}=t;if(r&&i)for(let t in r){let n=r[t],a=_T(i,t);a&&yT(e,a,n)}}function hT(e,t){let n=t.schema;if(!n)return;let r=n.classes,i=t.featureTables;if(r&&i)for(let t in r){let r=gT(i,t);r&&vT(e,n,r)}}function gT(e,t){for(let n in e){let r=e[n];if(r.class===t)return r}return null}function _T(e,t){for(let n in e){let r=e[n];if(r.class===t)return r}return null}function vT(e,t,n){if(!n.class)return;let r=t.classes?.[n.class];if(!r)throw Error(`Incorrect data in the EXT_structural_metadata extension: no schema class with name ${n.class}`);let i=n.count;for(let a in r.properties){let o=r.properties[a],s=n.properties?.[a];s&&(s.data=bT(e,t,o,i,s))}}function yT(e,t,n){let r=t.class;for(let i in n.properties){let n=t?.properties?.[i];n&&(n.data=TT(e,n,r))}}function bT(e,t,n,r,i){let a=[],o=i.bufferView,s=e.getTypedArrayForBufferView(o),c=xT(e,n,i,r),l=ST(e,n,i,r);return n.type===`STRING`||n.componentType===`STRING`?a=yw(r,s,c,l):CT(n)&&(a=wT(n,r,s,c)),a}function xT(e,t,n,r){return t.type===`ARRAY`&&t.componentCount===void 0&&n.arrayOffsetBufferView!==void 0?dw(e,n.arrayOffsetBufferView,n.offsetType||`UINT32`,r):null}function ST(e,t,n,r){return n.stringOffsetBufferView===void 0?null:dw(e,n.stringOffsetBufferView,n.offsetType||`UINT32`,r)}function CT(e){let t=[`UINT8`,`INT16`,`UINT16`,`INT32`,`UINT32`,`INT64`,`UINT64`,`FLOAT32`,`FLOAT64`];return t.includes(e.type)||e.componentType!==void 0&&t.includes(e.componentType)}function wT(e,t,n,r){let i=e.type===`ARRAY`,a=e.componentCount,o=`SCALAR`,s=e.componentType||e.type,c=uw(o,s),l=fw(n,o,s,n.byteLength/c);return i?r?_w(l,t,r,n.length,c):a?vw(l,t,a):[]:l}function TT(e,t,n){let r=e.gltf.json;if(!r.meshes)return[];let i=[];for(let a of r.meshes)for(let r of a.primitives)ET(e,n,t,i,r);return i}function ET(e,t,n,r,i){let a=pw(e,{channels:n.channels,...n.texture},i);a&&mw(e,t,a,r,i)}var DT=`4.5.4`,OT=`4.5.4`;function kT(){return globalThis._loadersgl_?.version||(globalThis._loadersgl_=globalThis._loadersgl_||{},globalThis._loadersgl_.version=`4.5.4`),globalThis._loadersgl_.version}var AT=kT();function jT(e,t){if(!e)throw Error(t||`loaders.gl assertion failed.`)}var MT={self:typeof self<`u`&&self,window:typeof window<`u`&&window,global:typeof global<`u`&&global,document:typeof document<`u`&&document};MT.self||MT.window||MT.global,MT.window||MT.self||MT.global,MT.global||MT.self||MT.window,MT.document;var NT=typeof process!=`object`||String(process)!==`[object process]`||process.browser,PT=typeof importScripts==`function`;typeof window<`u`&&window.orientation;var FT=typeof process<`u`&&process.version&&/v([0-9]*)/.exec(process.version);FT&&parseFloat(FT[1]);var IT={};function LT(e={}){let t=e.useLocalLibraries??e.core?.useLocalLibraries,n=e.CDN??e.core?.CDN,r=e.modules;return{...t===void 0?{}:{useLocalLibraries:t},...n===void 0?{}:{CDN:n},...r===void 0?{}:{modules:r}}}async function RT(e,t=null,n={},r=null){if(t){let i=n.modules?.[r||e];if(i!==void 0&&typeof i!=`string`)return i;e=zT(e,t,n,r)}return IT[e]=IT[e]||BT(e),await IT[e]}function zT(e,t,n={},r=null){if(n?.core)throw Error(`loadLibrary: options.core must be pre-normalized`);r||=e;let i=n.modules||{};return typeof i[r]==`string`?i[r]:!n.useLocalLibraries&&e.startsWith(`http`)?e:NT?n.CDN?(jT(n.CDN.startsWith(`http`)),`${n.CDN}/${t}@${AT}/dist/libs/${r}`):PT?`../src/libs/${r}`:`modules/${t}/src/libs/${r}`:`modules/${t}/dist/libs/${r}`}async function BT(e){if(e.endsWith(`wasm`))return await HT(e);if(!NT){let{requireFromFile:t}=globalThis.loaders||{};try{let n=await t?.(e);return n||!e.includes(`/dist/libs/`)?n:await t?.(e.replace(`/dist/libs/`,`/src/libs/`))}catch(n){if(e.includes(`/dist/libs/`))try{return await t?.(e.replace(`/dist/libs/`,`/src/libs/`))}catch{}return console.error(n),null}}return PT?importScripts(e):VT(await UT(e),e)}function VT(e,t){if(!NT){let{requireFromString:n}=globalThis.loaders||{};return n?.(e,t)}if(PT)return eval.call(globalThis,e),null;let n=document.createElement(`script`);n.id=t;try{n.appendChild(document.createTextNode(e))}catch{n.text=e}return document.body.appendChild(n),null}async function HT(e){let{readFileAsArrayBuffer:t}=globalThis.loaders||{};if(NT||!t||e.startsWith(`http`))return await(await fetch(e)).arrayBuffer();try{return await t(e)}catch{if(e.includes(`/dist/libs/`))return await t(e.replace(`/dist/libs/`,`/src/libs/`));throw Error(`Failed to load ArrayBuffer from ${e}`)}}async function UT(e){let{readFileAsText:t}=globalThis.loaders||{};if(NT||!t||e.startsWith(`http`))return await(await fetch(e)).text();try{return await t(e)}catch{if(e.includes(`/dist/libs/`))return await t(e.replace(`/dist/libs/`,`/src/libs/`));throw Error(`Failed to load text from ${e}`)}}function WT(e){globalThis.loaders||={},globalThis.loaders.modules||={},Object.assign(globalThis.loaders.modules,e)}function GT(e){return globalThis.loaders?.modules?.[e]||null}function KT(e,t){return new Promise((n,r)=>{let i=!1;function a(e){i||(i=!0,r(e))}function o(e){if(!i){i=!0;try{n(t(e))}catch(e){r(e)}}}try{e(o,a)?.then(o,a)}catch(e){a(e)}})}var qT={TRANSCODER:`basis_transcoder.js`,TRANSCODER_WASM:`basis_transcoder.wasm`,ENCODER:`basis_encoder.js`,ENCODER_WASM:`basis_encoder.wasm`},JT;async function YT(e){return WT(e.modules),GT(`basis`)||(JT||=XT(e),await JT)}async function XT(e){let t=null,n=null;return[t,n]=await Promise.all([await RT(qT.TRANSCODER,`textures`,e),await RT(qT.TRANSCODER_WASM,`textures`,e)]),t||=globalThis.BASIS,await ZT(t,n)}function ZT(e,t){let n={};return t&&(n.wasmBinary=t),KT(()=>e(n),e=>{let{BasisFile:t,initializeBasis:n}=e;return n(),{BasisFile:t}})}var QT;async function $T(e){let t=e.modules||{};return t.basisEncoder?t.basisEncoder:(QT||=eE(e),await QT)}async function eE(e){let t=null,n=null;return[t,n]=await Promise.all([await RT(qT.ENCODER,`textures`,e),await RT(qT.ENCODER_WASM,`textures`,e)]),t||=globalThis.BASIS,await tE(t,n)}function tE(e,t){let n={};return t&&(n.wasmBinary=t),KT(()=>e(n),e=>{let{BasisFile:t,KTX2File:n,BasisEncoder:r,initializeBasis:i}=e;return i(),{BasisFile:t,KTX2File:n,BasisEncoder:r}})}var nE=32854,rE=32856,iE=36194,aE=33776,oE=33779,sE=37493,cE=35840,lE=35842,uE=36196,dE=35986,fE=34798,pE=37808,mE=36283,hE=36285,gE=36492,_E=[``,`WEBKIT_`,`MOZ_`],vE={WEBGL_compressed_texture_s3tc:[`bc1-rgb-unorm-webgl`,`bc1-rgba-unorm`,`bc2-rgba-unorm`,`bc3-rgba-unorm`],WEBGL_compressed_texture_s3tc_srgb:[`bc1-rgb-unorm-srgb-webgl`,`bc1-rgba-unorm-srgb`,`bc2-rgba-unorm-srgb`,`bc3-rgba-unorm-srgb`],EXT_texture_compression_rgtc:[`bc4-r-unorm`,`bc4-r-snorm`,`bc5-rg-unorm`,`bc5-rg-snorm`],EXT_texture_compression_bptc:[`bc6h-rgb-ufloat`,`bc6h-rgb-float`,`bc7-rgba-unorm`,`bc7-rgba-unorm-srgb`],WEBGL_compressed_texture_etc1:[`etc1-rgb-unorm-webgl`],WEBGL_compressed_texture_etc:[`etc2-rgb8unorm`,`etc2-rgb8unorm-srgb`,`etc2-rgb8a1unorm`,`etc2-rgb8a1unorm-srgb`,`etc2-rgba8unorm`,`etc2-rgba8unorm-srgb`,`eac-r11unorm`,`eac-r11snorm`,`eac-rg11unorm`,`eac-rg11snorm`],WEBGL_compressed_texture_pvrtc:[`pvrtc-rgb4unorm-webgl`,`pvrtc-rgba4unorm-webgl`,`pvrtc-rgb2unorm-webgl`,`pvrtc-rgba2unorm-webgl`],WEBGL_compressed_texture_atc:[`atc-rgb-unorm-webgl`,`atc-rgba-unorm-webgl`,`atc-rgbai-unorm-webgl`],WEBGL_compressed_texture_astc:`astc-4x4-unorm.astc-4x4-unorm-srgb.astc-5x4-unorm.astc-5x4-unorm-srgb.astc-5x5-unorm.astc-5x5-unorm-srgb.astc-6x5-unorm.astc-6x5-unorm-srgb.astc-6x6-unorm.astc-6x6-unorm-srgb.astc-8x5-unorm.astc-8x5-unorm-srgb.astc-8x6-unorm.astc-8x6-unorm-srgb.astc-8x8-unorm.astc-8x8-unorm-srgb.astc-10x5-unorm.astc-10x5-unorm-srgb.astc-10x6-unorm.astc-10x6-unorm-srgb.astc-10x8-unorm.astc-10x8-unorm-srgb.astc-10x10-unorm.astc-10x10-unorm-srgb.astc-12x10-unorm.astc-12x10-unorm-srgb.astc-12x12-unorm.astc-12x12-unorm-srgb`.split(`.`)},yE=null;function bE(e){if(!yE){e=e||xE()||void 0,yE=new Set;for(let t of _E)for(let n in vE)if(e&&e.getExtension(`${t}${n}`))for(let e of vE[n])yE.add(e)}return yE}function xE(){try{return document.createElement(`canvas`).getContext(`webgl`)}catch{return null}}var SE=[171,75,84,88,32,50,48,187,13,10,26,10];function CE(e){let t=new Uint8Array(e);return!(t.byteLength<SE.length||t[0]!==SE[0]||t[1]!==SE[1]||t[2]!==SE[2]||t[3]!==SE[3]||t[4]!==SE[4]||t[5]!==SE[5]||t[6]!==SE[6]||t[7]!==SE[7]||t[8]!==SE[8]||t[9]!==SE[9]||t[10]!==SE[10]||t[11]!==SE[11])}var wE=Promise.resolve(),TE={etc1:{basisFormat:0,compressed:!0,format:uE,textureFormat:`etc1-rgb-unorm-webgl`},etc2:{basisFormat:1,compressed:!0,format:sE,textureFormat:`etc2-rgba8unorm`},bc1:{basisFormat:2,compressed:!0,format:aE,textureFormat:`bc1-rgb-unorm-webgl`},bc3:{basisFormat:3,compressed:!0,format:oE,textureFormat:`bc3-rgba-unorm`},bc4:{basisFormat:4,compressed:!0,format:mE,textureFormat:`bc4-r-unorm`},bc5:{basisFormat:5,compressed:!0,format:hE,textureFormat:`bc5-rg-unorm`},"bc7-m6-opaque-only":{basisFormat:6,compressed:!0,format:gE,textureFormat:`bc7-rgba-unorm`},"bc7-m5":{basisFormat:6,compressed:!0,format:gE,textureFormat:`bc7-rgba-unorm`},"pvrtc1-4-rgb":{basisFormat:8,compressed:!0,format:cE,textureFormat:`pvrtc-rgb4unorm-webgl`},"pvrtc1-4-rgba":{basisFormat:9,compressed:!0,format:lE,textureFormat:`pvrtc-rgba4unorm-webgl`},"astc-4x4":{basisFormat:10,compressed:!0,format:pE,textureFormat:`astc-4x4-unorm`},"atc-rgb":{basisFormat:11,compressed:!0,format:dE,textureFormat:`atc-rgb-unorm-webgl`},"atc-rgba-interpolated-alpha":{basisFormat:12,compressed:!0,format:fE,textureFormat:`atc-rgbai-unorm-webgl`},rgba32:{basisFormat:13,compressed:!1,format:rE,textureFormat:`rgba8unorm`},rgb565:{basisFormat:14,compressed:!1,format:iE,textureFormat:`rgb565unorm-webgl`},bgr565:{basisFormat:15,compressed:!1,format:iE,textureFormat:`rgb565unorm-webgl`},rgba4444:{basisFormat:16,compressed:!1,format:nE,textureFormat:`rgba4unorm-webgl`}};Object.freeze(Object.keys(TE));async function EE(e){let t=wE,n;wE=new Promise(e=>{n=e}),await t;try{return await e()}finally{n()}}async function DE(e,t={}){let n=LT(t);return await EE(async()=>{if(!t.basis?.containerFormat||t.basis.containerFormat===`auto`){if(CE(e))return AE((await $T(n)).KTX2File,e,t);let{BasisFile:r}=await YT(n);return OE(r,e,t)}switch(t.basis.module){case`encoder`:let r=await $T(n);switch(t.basis.containerFormat){case`ktx2`:return AE(r.KTX2File,e,t);default:return OE(r.BasisFile,e,t)}default:let{BasisFile:i}=await YT(n);return OE(i,e,t)}})}function OE(e,t,n){let r=new e(new Uint8Array(t));try{if(!r.startTranscoding())throw Error(`Failed to start basis transcoding`);let e=r.getNumImages(),t=[];for(let i=0;i<e;i++){let e=r.getNumLevels(i),a=[];for(let t=0;t<e;t++)a.push(kE(r,i,t,n));t.push(a)}return t}finally{r.close(),r.delete()}}function kE(e,t,n,r){let i=e.getImageWidth(t,n),a=e.getImageHeight(t,n),o=e.getHasAlpha(),{compressed:s,format:c,basisFormat:l,textureFormat:u}=ME(r,o),d=e.getImageTranscodedSizeInBytes(t,n,l),f=new Uint8Array(d);if(!e.transcodeImage(f,t,n,l,0,0))throw Error(`failed to start Basis transcoding`);return{shape:`texture-level`,width:i,height:a,data:f,compressed:s,...c===void 0?{}:{format:c},...u===void 0?{}:{textureFormat:u},hasAlpha:o}}function AE(e,t,n){let r=new e(new Uint8Array(t));try{if(!r.startTranscoding())throw Error(`failed to start KTX2 transcoding`);let e=r.getLevels(),t=[];for(let i=0;i<e;i++)t.push(jE(r,i,n));return[t]}finally{r.close(),r.delete()}}function jE(e,t,n){let{alphaFlag:r,height:i,width:a}=e.getImageLevelInfo(t,0,0),{compressed:o,format:s,basisFormat:c,textureFormat:l}=ME(n,r),u=e.getImageTranscodedSizeInBytes(t,0,0,c),d=new Uint8Array(u);if(!e.transcodeImage(d,t,0,0,c,0,-1,-1))throw Error(`Failed to transcode KTX2 image`);return{shape:`texture-level`,width:a,height:i,data:d,compressed:o,...s===void 0?{}:{format:s},...l===void 0?{}:{textureFormat:l},levelSize:u,hasAlpha:r}}function ME(e,t){let n=e.basis?.format||`auto`;n===`auto`&&(n=e.basis?.supportedTextureFormats?NE(e.basis.supportedTextureFormats):NE()),typeof n==`object`&&(n=t?n.alpha:n.noAlpha);let r=TE[n.toLowerCase()];if(!r)throw Error(`Unknown Basis format ${n}`);return r}function NE(e=bE()){let t=new Set(e);return PE(t,[`astc-4x4-unorm`,`astc-4x4-unorm-srgb`])?`astc-4x4`:PE(t,[`bc7-rgba-unorm`,`bc7-rgba-unorm-srgb`])?{alpha:`bc7-m5`,noAlpha:`bc7-m6-opaque-only`}:PE(t,[`bc1-rgb-unorm-webgl`,`bc1-rgb-unorm-srgb-webgl`,`bc1-rgba-unorm`,`bc1-rgba-unorm-srgb`,`bc2-rgba-unorm`,`bc2-rgba-unorm-srgb`,`bc3-rgba-unorm`,`bc3-rgba-unorm-srgb`])?{alpha:`bc3`,noAlpha:`bc1`}:PE(t,[`pvrtc-rgb4unorm-webgl`,`pvrtc-rgba4unorm-webgl`,`pvrtc-rgb2unorm-webgl`,`pvrtc-rgba2unorm-webgl`])?{alpha:`pvrtc1-4-rgba`,noAlpha:`pvrtc1-4-rgb`}:PE(t,[`etc2-rgb8unorm`,`etc2-rgb8unorm-srgb`,`etc2-rgb8a1unorm`,`etc2-rgb8a1unorm-srgb`,`etc2-rgba8unorm`,`etc2-rgba8unorm-srgb`,`eac-r11unorm`,`eac-r11snorm`,`eac-rg11unorm`,`eac-rg11snorm`])?`etc2`:t.has(`etc1-rgb-unorm-webgl`)?`etc1`:PE(t,[`atc-rgb-unorm-webgl`,`atc-rgba-unorm-webgl`,`atc-rgbai-unorm-webgl`])?{alpha:`atc-rgba-interpolated-alpha`,noAlpha:`atc-rgb`}:`rgb565`}function PE(e,t){return t.some(t=>e.has(t))}var FE={dataType:null,batchType:null,name:`Basis`,id:`basis`,module:`textures`,version:OT,worker:!0,extensions:[`basis`,`ktx2`],mimeTypes:[`application/octet-stream`,`image/ktx2`],tests:[`sB`],binary:!0,options:{basis:{format:`auto`,containerFormat:`auto`,module:`transcoder`}},parse:DE},IE=!0,LE=1735152710,RE=12,zE=8,BE=1313821514,VE=5130562,HE=0,UE=0,WE=1;function GE(e,t=0){return`\
${String.fromCharCode(e.getUint8(t+0))}\
${String.fromCharCode(e.getUint8(t+1))}\
${String.fromCharCode(e.getUint8(t+2))}\
${String.fromCharCode(e.getUint8(t+3))}`}function KE(e,t=0,n={}){let r=new DataView(e),{magic:i=LE}=n,a=r.getUint32(t,!1);return a===i||a===LE}function qE(e,t,n=0,r={}){let i=new DataView(t),a=GE(i,n+0),o=i.getUint32(n+4,IE),s=i.getUint32(n+8,IE);switch(Object.assign(e,{header:{byteOffset:n,byteLength:s,hasBinChunk:!1},type:a,version:o,json:{},binChunks:[]}),n+=RE,e.version){case 1:return JE(e,i,n);case 2:return YE(e,i,n,r={});default:throw Error(`Invalid GLB version ${e.version}. Only supports version 1 and 2.`)}}function JE(e,t,n){PS(e.header.byteLength>20);let r=t.getUint32(n+0,IE),i=t.getUint32(n+4,IE);return n+=zE,PS(i===HE),ZE(e,t,n,r),n+=r,n+=QE(e,t,n,e.header.byteLength),n}function YE(e,t,n,r){return PS(e.header.byteLength>20),XE(e,t,n,r),n+e.header.byteLength}function XE(e,t,n,r){for(;n+8<=e.header.byteLength;){let i=t.getUint32(n+0,IE),a=t.getUint32(n+4,IE);switch(n+=zE,a){case BE:ZE(e,t,n,i);break;case VE:QE(e,t,n,i);break;case UE:r.strict||ZE(e,t,n,i);break;case WE:r.strict||QE(e,t,n,i);break;default:break}n+=US(i,4)}return n}function ZE(e,t,n,r){let i=new Uint8Array(t.buffer,n,r),a=new TextDecoder(`utf8`).decode(i);return e.json=JSON.parse(a),US(r,4)}function QE(e,t,n,r){return e.header.hasBinChunk=!0,e.binChunks.push({byteOffset:n,byteLength:r,arrayBuffer:t.buffer}),US(r,4)}function $E(e,t,n){if(e.startsWith(`data:`)||e.startsWith(`http:`)||e.startsWith(`https:`))return e;let r=n?.baseUrl||eD(t?.core?.baseUrl);if(!r)throw Error(`'baseUrl' must be provided to resolve relative url ${e}`);return r.endsWith(`/`)?`${r}${e}`:`${r}/${e}`}function eD(e){if(!e)return;if(e.endsWith(`/`))return e;let t=e.lastIndexOf(`/`);return t>=0?e.slice(0,t+1):``}var tD=(function(){var e=`b9H79Tebbbe8Fv9Gbb9Gvuuuuueu9Giuuub9Geueu9Giuuueuixkbeeeddddillviebeoweuecj:Gdkr;Neqo9TW9T9VV95dbH9F9F939H79T9F9J9H229F9Jt9VV7bb8A9TW79O9V9Wt9F9KW9J9V9KW9wWVtW949c919M9MWVbeY9TW79O9V9Wt9F9KW9J9V9KW69U9KW949c919M9MWVbdE9TW79O9V9Wt9F9KW9J9V9KW69U9KW949tWG91W9U9JWbiL9TW79O9V9Wt9F9KW9J9V9KWS9P2tWV9p9JtblK9TW79O9V9Wt9F9KW9J9V9KWS9P2tWV9r919HtbvL9TW79O9V9Wt9F9KW9J9V9KWS9P2tWVT949WboY9TW79O9V9Wt9F9KW9J9V9KWS9P2tWVJ9V29VVbrl79IV9Rbwq;X8:kdbk:kYi5ud9:du8Jjjjjbcjq9Rgv8Kjjjjbc9:hodnalTmbcuhoaiRbbgrc;WeGc:Ge9hmbarcsGgwce0mbc9:hoalcufadcd4cbawEgDadfgrcKcaawEgqaraq0Egk6mbaicefhxcj;abad9Uc;WFbGcjdadca0EhmaialfgPar9Rgoadfhsavaoadz:jjjjbgzceVhHcbhOdndninaeaO9nmeaPax9RaD6mdamaeaO9RaOamfgoae6EgAcsfglc9WGhCaAcethXaxaDfhiaOaeaoaeao6E9RhQalcl4cifcd4hLazcjdfaAfhKcbhYabaOad2fg8AhEaHh3incbh5dnawTmbaxaYcd4fRbbh5kcbh8Eazcjdfhqinaih8Fdndndndna5a8Ecet4ciGgoc9:fPdebdkaPa8F9RaA6mrazcjdfa8EaA2fa8FaAz:jjjjb8Aa8FaAfhixdkazcjdfa8EaA2fcbaAz:kjjjb8Aa8FhixekaPa8F9RaL6mva8FaLfhidnaCTmbaPai9RcK6mbaocdtc:q:G:cjbfcj:G:cjbawEhaczhrcbhlinargoc9Wfghaqfhrdndndndndndnaaa8Fahco4fRbbalcoG4ciGcdtfydbPDbedvivvvlvkar9cb83bwar9cb83bbxlkarcbaiRbdai8Xbb9c:c:qj:bw9:9c:q;c1:I1e:d9c:b:c:e1z9:gg9cjjjjjz:dg8J9qE86bbaqaofgrcGfcbaicdfa8J9c8N1:NfghRbbag9cjjjjjw:dg8J9qE86bbarcVfcbaha8J9c8M1:NfghRbbag9cjjjjjl:dg8J9qE86bbarc7fcbaha8J9c8L1:NfghRbbag9cjjjjjd:dg8J9qE86bbarctfcbaha8J9c8K1:NfghRbbag9cjjjjje:dg8J9qE86bbarc91fcbaha8J9c8J1:NfghRbbag9cjjjj;ab:dg8J9qE86bbarc4fcbaha8J9cg1:NfghRbbag9cjjjja:dg8J9qE86bbarc93fcbaha8J9ch1:NfghRbbag9cjjjjz:dgg9qE86bbarc94fcbahag9ca1:NfghRbbai8Xbe9c:c:qj:bw9:9c:q;c1:I1e:d9c:b:c:e1z9:gg9cjjjjjz:dg8J9qE86bbarc95fcbaha8J9c8N1:NfgiRbbag9cjjjjjw:dg8J9qE86bbarc96fcbaia8J9c8M1:NfgiRbbag9cjjjjjl:dg8J9qE86bbarc97fcbaia8J9c8L1:NfgiRbbag9cjjjjjd:dg8J9qE86bbarc98fcbaia8J9c8K1:NfgiRbbag9cjjjjje:dg8J9qE86bbarc99fcbaia8J9c8J1:NfgiRbbag9cjjjj;ab:dg8J9qE86bbarc9:fcbaia8J9cg1:NfgiRbbag9cjjjja:dg8J9qE86bbarcufcbaia8J9ch1:NfgiRbbag9cjjjjz:dgg9qE86bbaiag9ca1:NfhixikaraiRblaiRbbghco4g8Ka8KciSg8KE86bbaqaofgrcGfaiclfa8Kfg8KRbbahcl4ciGg8La8LciSg8LE86bbarcVfa8Ka8Lfg8KRbbahcd4ciGg8La8LciSg8LE86bbarc7fa8Ka8Lfg8KRbbahciGghahciSghE86bbarctfa8Kahfg8KRbbaiRbeghco4g8La8LciSg8LE86bbarc91fa8Ka8Lfg8KRbbahcl4ciGg8La8LciSg8LE86bbarc4fa8Ka8Lfg8KRbbahcd4ciGg8La8LciSg8LE86bbarc93fa8Ka8Lfg8KRbbahciGghahciSghE86bbarc94fa8Kahfg8KRbbaiRbdghco4g8La8LciSg8LE86bbarc95fa8Ka8Lfg8KRbbahcl4ciGg8La8LciSg8LE86bbarc96fa8Ka8Lfg8KRbbahcd4ciGg8La8LciSg8LE86bbarc97fa8Ka8Lfg8KRbbahciGghahciSghE86bbarc98fa8KahfghRbbaiRbigico4g8Ka8KciSg8KE86bbarc99faha8KfghRbbaicl4ciGg8Ka8KciSg8KE86bbarc9:faha8KfghRbbaicd4ciGg8Ka8KciSg8KE86bbarcufaha8KfgrRbbaiciGgiaiciSgiE86bbaraifhixdkaraiRbwaiRbbghcl4g8Ka8KcsSg8KE86bbaqaofgrcGfaicwfa8Kfg8KRbbahcsGghahcsSghE86bbarcVfa8KahfghRbbaiRbeg8Kcl4g8La8LcsSg8LE86bbarc7faha8LfghRbba8KcsGg8Ka8KcsSg8KE86bbarctfaha8KfghRbbaiRbdg8Kcl4g8La8LcsSg8LE86bbarc91faha8LfghRbba8KcsGg8Ka8KcsSg8KE86bbarc4faha8KfghRbbaiRbig8Kcl4g8La8LcsSg8LE86bbarc93faha8LfghRbba8KcsGg8Ka8KcsSg8KE86bbarc94faha8KfghRbbaiRblg8Kcl4g8La8LcsSg8LE86bbarc95faha8LfghRbba8KcsGg8Ka8KcsSg8KE86bbarc96faha8KfghRbbaiRbvg8Kcl4g8La8LcsSg8LE86bbarc97faha8LfghRbba8KcsGg8Ka8KcsSg8KE86bbarc98faha8KfghRbbaiRbog8Kcl4g8La8LcsSg8LE86bbarc99faha8LfghRbba8KcsGg8Ka8KcsSg8KE86bbarc9:faha8KfghRbbaiRbrgicl4g8Ka8KcsSg8KE86bbarcufaha8KfgrRbbaicsGgiaicsSgiE86bbaraifhixekarai8Pbw83bwarai8Pbb83bbaiczfhikdnaoaC9pmbalcdfhlaoczfhraPai9RcL0mekkaoaC6moaimexokaCmva8FTmvkaqaAfhqa8Ecefg8Ecl9hmbkdndndndnawTmbasaYcd4fRbbgociGPlbedrbkaATmdazaYfh8Fazcjdfhhcbh8EaEhaina8FRbbhraahocbhlinaoahalfRbbgqce4cbaqceG9R7arfgr86bbaoadfhoaAalcefgl9hmbkaacefhaa8Fcefh8FahaAfhha8Ecefg8Ecl9hmbxikkaATmeazaYfhaazcjdfhhcbhoceh8EaKh8FinaEaofhlaa8Vbbhrcbhoinala8FaofRbbcwtahaofRbbgqVc;:FiGce4cbaqceG9R7arfgr87bbaladfhlaQaocefgofmbka8FaXfh8FcdhoaacdfhaahaXfhha8EceGhlcbh8EalmbxdkkaATmbaocl4h8EazaYfRbbhqcwhoa3hlinalRbbaotaqVhqalcefhlaocwfgoca9hmbkcbhhaEh8FaKhainazcjdfahfRbbhrcwhoaahlinalRbbaotarVhralaAfhlaocwfgoca9hmbkara8E94aq7hqcbhoa8Fhlinalaqao486bbalcefhlaocwfgoca9hmbka8Fadfh8FaacefhaahcefghaA9hmbkkaEclfhEa3clfh3aYclfgYad6mbkaza8AaAcufad2fadz:jjjjb8AaAaOfhOaihxaimbkc9:hoxdkcbc99aPax9RakSEhoxekc9:hokavcjqf8Kjjjjbaok:bsesu8Jjjjjbc;ae9Rgv8Kjjjjbc9:hodnalaeci9UgrcHf6mbcuhoaiRbbgwc;WeGc;Ge9hmbawcsGgwce0mbavc;abfcFecjez:kjjjb8Aav9cu83iUav9cu83i8Wav9cu83iyav9cu83iaav9cu83iKav9cu83izav9cu83iwav9cu83ibaialfc9WfhDaicefgiarfhqdndnaeci9pmbaqhexekcmcsawceSEhkcbhxcbhmaqhecbhlcbhoindndnaiRbbgrc;Ve0mbavc;abfaoarcu7gPcl4fcsGcitfgwydlhsawydbhzdndnarcsGgwak9pmbavalaPfcsGcdtfydbaxawEhraxawTgHfhxxekdnaeaD9nmbc9:hoxokdndnawcsSmbcehHamawcetfcWfhrxekaecefhrae8SbbgwcFeGhPdndnawcu9mmbarhexekaecvfheaPcFbGhPcrhwdninar8SbbgHcFbGawtaPVhPaHcu9kmearcefhrawcrfgwc8J9hmbxdkkarcefhekcehHaPce4cbaPceG9R7amfhrkarhmkavc;abfaocitfgwarBdbawasBdlavalcdtfarBdbavc;abfaocefcsGcitfgwazBdbawarBdlaocdfhoaHalfhldnadcd9hmbabar87elabas87edabaz87ebabcofhbxdkabarBdwabasBdlabazBdbabcxfhbxekdnarcpe0mbavalaDarcsGfRbbgwcl4gP9RcsGcdtfydbaxcefgsaPEhravalaw9RcsGcdtfydbasaPTgzfgsawcsGgPEhwaPThPdndnadcd9hmbabaw87elabar87edabax87ebcohHxekabawBdwabarBdlabaxBdbcxhHkavalcdtfaxBdbavc;abfaocitfgOarBdbaOaxBdlavalcefglcsGcdtfarBdbavc;abfaocefcsGcitfgOawBdbaOarBdlavalazfglcsGcdtfawBdbavc;abfaocdfcsGcitfgraxBdbarawBdlaocifhoabaHfhbalaPfhlasaPfhxxekdnaeaD9nmbc9:hoxlkaxcbaeRbbgwEgHarc;:eSgrfhsawcsGhOdndnawcl4gAmbascefhzxekashzavalaA9RcsGcdtfydbhskdndnaOmbazcefhxxekazhxavalaw9RcsGcdtfydbhzkdndnarTmbaecefhrxekaecdfhrae8SbegPcFeGhwdnaPcu9kmbaecofhHawcFbGhwcrhedninar8SbbgPcFbGaetawVhwaPcu9kmearcefhraecrfgec8J9hmbkaHhrxekarcefhrkawce4cbawceG9R7amfgmhHkdndnaAcsSmbarhwxekarcefhwar8SbbgecFeGhPdnaecu9kmbarcvfhsaPcFbGhPcrhedninaw8SbbgrcFbGaetaPVhParcu9kmeawcefhwaecrfgec8J9hmbkashwxekawcefhwkaPce4cbaPceG9R7amfgmhskdndnaOcsSmbawhexekawcefheaw8SbbgrcFeGhPdnarcu9kmbawcvfhzaPcFbGhPcrhrdninae8SbbgwcFbGartaPVhPawcu9kmeaecefhearcrfgrc8J9hmbkazhexekaecefhekaPce4cbaPceG9R7amfgmhzkdndnadcd9hmbabaz87elabas87edabaH87ebcohrxekabazBdwabasBdlabaHBdbcxhrkavc;abfaocitfgwasBdbawaHBdlavalcdtfaHBdbavc;abfaocefcsGcitfgwazBdbawasBdlavalcefglcsGcdtfasBdbavc;abfaocdfcsGcitfgwaHBdbawazBdlavalaATaAcsSVfglcsGcdtfazBdbalaOTaOcsSVfhlaocifhoabarfhbkaocsGhoalcsGhlaicefgiaq6mbkkcbc99aeaDSEhokavc;aef8Kjjjjbaok:clevu8Jjjjjbcz9Rhvdnalaecvf9pmbc9:skdnaiRbbc;:eGc;qeSmbcuskav9cb83iwaicefhoaialfc98fhrdnaeTmbdnadcdSmbcbhwindnaoar6mbc9:skaocefhlao8SbbgicFeGhddndnaicu9mmbalhoxekaocvfhoadcFbGhdcrhidninal8SbbgDcFbGaitadVhdaDcu9kmealcefhlaicrfgic8J9hmbxdkkalcefhokabawcdtfadc8Etc8F91adcd47avcwfadceGcdtVglydbfgiBdbalaiBdbawcefgwae9hmbxdkkcbhwindnaoar6mbc9:skaocefhlao8SbbgicFeGhddndnaicu9mmbalhoxekaocvfhoadcFbGhdcrhidninal8SbbgDcFbGaitadVhdaDcu9kmealcefhlaicrfgic8J9hmbxdkkalcefhokabawcetfadc8Etc8F91adcd47avcwfadceGcdtVglydbfgi87ebalaiBdbawcefgwae9hmbkkcbc99aoarSEk:Lvoeue99dud99eud99dndnadcl9hmbaeTmeindndnabcdfgd8Sbb:Yab8Sbbgi:Ygl:l:tabcefgv8Sbbgo:Ygr:l:tgwJbb;:9cawawNJbbbbawawJbbbb9GgDEgq:mgkaqaicb9iEalMgwawNakaqaocb9iEarMgqaqNMM:r:vglNJbbbZJbbb:;aDEMgr:lJbbb9p9DTmbar:Ohixekcjjjj94hikadai86bbdndnaqalNJbbbZJbbb:;aqJbbbb9GEMgq:lJbbb9p9DTmbaq:Ohdxekcjjjj94hdkavad86bbdndnawalNJbbbZJbbb:;awJbbbb9GEMgw:lJbbb9p9DTmbaw:Ohdxekcjjjj94hdkabad86bbabclfhbaecufgembxdkkaeTmbindndnabclfgd8Ueb:Yab8Uebgi:Ygl:l:tabcdfgv8Uebgo:Ygr:l:tgwJb;:FSawawNJbbbbawawJbbbb9GgDEgq:mgkaqaicb9iEalMgwawNakaqaocb9iEarMgqaqNMM:r:vglNJbbbZJbbb:;aDEMgr:lJbbb9p9DTmbar:Ohixekcjjjj94hikadai87ebdndnaqalNJbbbZJbbb:;aqJbbbb9GEMgq:lJbbb9p9DTmbaq:Ohdxekcjjjj94hdkavad87ebdndnawalNJbbbZJbbb:;awJbbbb9GEMgw:lJbbb9p9DTmbaw:Ohdxekcjjjj94hdkabad87ebabcwfhbaecufgembkkk:4ioiue99dud99dud99dnaeTmbcbhiabhlindndnal8Uebgv:YgoJ:ji:1Salcof8UebgrciVgw:Y:vgDNJbbbZJbbb:;avcu9kEMgq:lJbbb9p9DTmbaq:Ohkxekcjjjj94hkkalclf8Uebhvalcdf8UebhxalarcefciGcetfak87ebdndnax:YgqaDNJbbbZJbbb:;axcu9kEMgm:lJbbb9p9DTmbam:Ohxxekcjjjj94hxkabaiarciGgkfcd7cetfax87ebdndnav:YgmaDNJbbbZJbbb:;avcu9kEMgP:lJbbb9p9DTmbaP:Ohvxekcjjjj94hvkalarcufciGcetfav87ebdndnawaw2:ZgPaPMaoaoN:taqaqN:tamamN:tgoJbbbbaoJbbbb9GE:raDNJbbbZMgD:lJbbb9p9DTmbaD:Ohrxekcjjjj94hrkalakcetfar87ebalcwfhlaiclfhiaecufgembkkk9mbdnadcd4ae2gdTmbinababydbgecwtcw91:Yaece91cjjj98Gcjjj;8if::NUdbabclfhbadcufgdmbkkk:Tvirud99eudndnadcl9hmbaeTmeindndnabRbbgiabcefgl8Sbbgvabcdfgo8Sbbgrf9R:YJbbuJabcifgwRbbgdce4adVgDcd4aDVgDcl4aDVgD:Z:vgqNJbbbZMgk:lJbbb9p9DTmbak:Ohxxekcjjjj94hxkaoax86bbdndnaraif:YaqNJbbbZMgk:lJbbb9p9DTmbak:Ohoxekcjjjj94hokalao86bbdndnavaifar9R:YaqNJbbbZMgk:lJbbb9p9DTmbak:Ohixekcjjjj94hikabai86bbdndnaDadcetGadceGV:ZaqNJbbbZMgq:lJbbb9p9DTmbaq:Ohdxekcjjjj94hdkawad86bbabclfhbaecufgembxdkkaeTmbindndnab8Vebgiabcdfgl8Uebgvabclfgo8Uebgrf9R:YJbFu9habcofgw8Vebgdce4adVgDcd4aDVgDcl4aDVgDcw4aDVgD:Z:vgqNJbbbZMgk:lJbbb9p9DTmbak:Ohxxekcjjjj94hxkaoax87ebdndnaraif:YaqNJbbbZMgk:lJbbb9p9DTmbak:Ohoxekcjjjj94hokalao87ebdndnavaifar9R:YaqNJbbbZMgk:lJbbb9p9DTmbak:Ohixekcjjjj94hikabai87ebdndnaDadcetGadceGV:ZaqNJbbbZMgq:lJbbb9p9DTmbaq:Ohdxekcjjjj94hdkawad87ebabcwfhbaecufgembkkk9teiucbcbyd:K:G:cjbgeabcifc98GfgbBd:K:G:cjbdndnabZbcztgd9nmbcuhiabad9RcFFifcz4nbcuSmekaehikaik;LeeeudndnaeabVciGTmbabhixekdndnadcz9pmbabhixekabhiinaiaeydbBdbaiclfaeclfydbBdbaicwfaecwfydbBdbaicxfaecxfydbBdbaeczfheaiczfhiadc9Wfgdcs0mbkkadcl6mbinaiaeydbBdbaeclfheaiclfhiadc98fgdci0mbkkdnadTmbinaiaeRbb86bbaicefhiaecefheadcufgdmbkkabk;aeedudndnabciGTmbabhixekaecFeGc:b:c:ew2hldndnadcz9pmbabhixekabhiinaialBdbaicxfalBdbaicwfalBdbaiclfalBdbaiczfhiadc9Wfgdcs0mbkkadcl6mbinaialBdbaiclfhiadc98fgdci0mbkkdnadTmbinaiae86bbaicefhiadcufgdmbkkabkk83dbcj:Gdk8Kbbbbdbbblbbbwbbbbbbbebbbdbbblbbbwbbbbc:K:Gdkl8W:qbb`,t=`b9H79TebbbeKl9Gbb9Gvuuuuueu9Giuuub9Geueuixkbbebeeddddilve9Weeeviebeoweuecj:Gdkr;Neqo9TW9T9VV95dbH9F9F939H79T9F9J9H229F9Jt9VV7bb8A9TW79O9V9Wt9F9KW9J9V9KW9wWVtW949c919M9MWVbdY9TW79O9V9Wt9F9KW9J9V9KW69U9KW949c919M9MWVblE9TW79O9V9Wt9F9KW9J9V9KW69U9KW949tWG91W9U9JWbvL9TW79O9V9Wt9F9KW9J9V9KWS9P2tWV9p9JtboK9TW79O9V9Wt9F9KW9J9V9KWS9P2tWV9r919HtbrL9TW79O9V9Wt9F9KW9J9V9KWS9P2tWVT949WbwY9TW79O9V9Wt9F9KW9J9V9KWS9P2tWVJ9V29VVbDl79IV9Rbqq:I9Dklbzik94evu8Jjjjjbcz9Rhbcbheincbhdcbhiinabcwfadfaicjuaead4ceGglE86bbaialfhiadcefgdcw9hmbkaeai86b:q:W:cjbaecitab8Piw83i:q:G:cjbaecefgecjd9hmbkk:SBlEud97dur978Jjjjjbcj;kb9Rgv8Kjjjjbc9:hodnalTmbcuhoaiRbbgrc;WeGc:Ge9hmbarcsGgwce0mbc9:hoalcufadcd4cbawEgDadfgrcKcaawEgqaraq0Egk6mbaialfgxar9RhodnadTgmmbavaoad;8qbbkaicefhPcj;abad9Uc;WFbGcjdadca0EhsdndndnadTmbaoadfhzcbhHinaeaH9nmdaxaP9RaD6miabaHad2fgOavcjdfasaeaH9RaHasfae6EgAaAcsfgoc9WGgCSEhXaPaDfhQaocl4cifcd4hLavcj;cbfaCcetfhKavcj;cbfaCci2fhYavcj;cbfaCfh8AcbhEaoc;ab6h3incbh5dnawTmbaPaEcd4fRbbh5kcbh8Eavcj;cbfh8Findndndndna5a8Ecet4ciGgoc9:fPdebdkaxaQ9RaC6mwdnaCTmbavcj;cbfa8EaC2faQaC;8qbbkaQaAfhQxdkaCTmeavcj;cbfa8EaC2fcbaC;8kbxekaxaQ9RaL6moaoclVcbawEhraQaLfhocbhidna3mbaxao9Rc;Gb6mbcbhlina8FalfhidndndndndndnaQalco4fRbbgqciGarfPDbedibledibkaipxbbbbbbbbbbbbbbbbpklbxlkaiaopbblaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLgacdp:meaapmbzeHdOiAlCvXoQrLpxiiiiiiiiiiiiiiiip9oghpxiiiiiiiiiiiiiiiip8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Nggcitpbi:q:G:cjbagRb:q:W:cjbggpsaap5e9cjF;8;4;W;G;ab9:9cU1:Ng8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spklbagaoclffa8JRb:q:W:cjbfhoxikaiaopbbwaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLpxssssssssssssssssp9oghpxssssssssssssssssp8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Nggcitpbi:q:G:cjbagRb:q:W:cjbggpsaap5e9cjF;8;4;W;G;ab9:9cU1:Ng8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spklbagaocwffa8JRb:q:W:cjbfhoxdkaiaopbbbpklbaoczfhoxekaiaopbbdaoRbbggcitpbi:q:G:cjbagRb:q:W:cjbggpsaoRbeg8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPpklbagaocdffa8JRb:q:W:cjbfhokdndndndndndnaqcd4ciGarfPDbedibledibkaiczfpxbbbbbbbbbbbbbbbbpklbxlkaiczfaopbblaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLgacdp:meaapmbzeHdOiAlCvXoQrLpxiiiiiiiiiiiiiiiip9oghpxiiiiiiiiiiiiiiiip8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Nggcitpbi:q:G:cjbagRb:q:W:cjbggpsaap5e9cjF;8;4;W;G;ab9:9cU1:Ng8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spklbagaoclffa8JRb:q:W:cjbfhoxikaiczfaopbbwaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLpxssssssssssssssssp9oghpxssssssssssssssssp8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Nggcitpbi:q:G:cjbagRb:q:W:cjbggpsaap5e9cjF;8;4;W;G;ab9:9cU1:Ng8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spklbagaocwffa8JRb:q:W:cjbfhoxdkaiczfaopbbbpklbaoczfhoxekaiczfaopbbdaoRbbggcitpbi:q:G:cjbagRb:q:W:cjbggpsaoRbeg8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPpklbagaocdffa8JRb:q:W:cjbfhokdndndndndndnaqcl4ciGarfPDbedibledibkaicafpxbbbbbbbbbbbbbbbbpklbxlkaicafaopbblaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLgacdp:meaapmbzeHdOiAlCvXoQrLpxiiiiiiiiiiiiiiiip9oghpxiiiiiiiiiiiiiiiip8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Nggcitpbi:q:G:cjbagRb:q:W:cjbggpsaap5e9cjF;8;4;W;G;ab9:9cU1:Ng8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spklbagaoclffa8JRb:q:W:cjbfhoxikaicafaopbbwaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLpxssssssssssssssssp9oghpxssssssssssssssssp8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Nggcitpbi:q:G:cjbagRb:q:W:cjbggpsaap5e9cjF;8;4;W;G;ab9:9cU1:Ng8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spklbagaocwffa8JRb:q:W:cjbfhoxdkaicafaopbbbpklbaoczfhoxekaicafaopbbdaoRbbggcitpbi:q:G:cjbagRb:q:W:cjbggpsaoRbeg8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPpklbagaocdffa8JRb:q:W:cjbfhokdndndndndndnaqco4arfPDbedibledibkaic8Wfpxbbbbbbbbbbbbbbbbpklbxlkaic8Wfaopbblaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLgacdp:meaapmbzeHdOiAlCvXoQrLpxiiiiiiiiiiiiiiiip9oghpxiiiiiiiiiiiiiiiip8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Ngicitpbi:q:G:cjbaiRb:q:W:cjbgipsaap5e9cjF;8;4;W;G;ab9:9cU1:Ngqcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spklbaiaoclffaqRb:q:W:cjbfhoxikaic8Wfaopbbwaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLpxssssssssssssssssp9oghpxssssssssssssssssp8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Ngicitpbi:q:G:cjbaiRb:q:W:cjbgipsaap5e9cjF;8;4;W;G;ab9:9cU1:Ngqcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spklbaiaocwffaqRb:q:W:cjbfhoxdkaic8Wfaopbbbpklbaoczfhoxekaic8WfaopbbdaoRbbgicitpbi:q:G:cjbaiRb:q:W:cjbgipsaoRbegqcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPpklbaiaocdffaqRb:q:W:cjbfhokalc;abfhialcjefaC0meaihlaxao9Rc;Fb0mbkkdnaiaC9pmbaici4hlinaxao9RcK6mwa8FaifhqdndndndndndnaQaico4fRbbalcoG4ciGarfPDbedibledibkaqpxbbbbbbbbbbbbbbbbpkbbxlkaqaopbblaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLgacdp:meaapmbzeHdOiAlCvXoQrLpxiiiiiiiiiiiiiiiip9oghpxiiiiiiiiiiiiiiiip8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Nggcitpbi:q:G:cjbagRb:q:W:cjbggpsaap5e9cjF;8;4;W;G;ab9:9cU1:Ng8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spkbbagaoclffa8JRb:q:W:cjbfhoxikaqaopbbwaopbbbgaclp:meaapmbzeHdOiAlCvXoQrLpxssssssssssssssssp9oghpxssssssssssssssssp8Jgap5b9cjF;8;4;W;G;ab9:9cU1:Nggcitpbi:q:G:cjbagRb:q:W:cjbggpsaap5e9cjF;8;4;W;G;ab9:9cU1:Ng8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPahaap9spkbbagaocwffa8JRb:q:W:cjbfhoxdkaqaopbbbpkbbaoczfhoxekaqaopbbdaoRbbggcitpbi:q:G:cjbagRb:q:W:cjbggpsaoRbeg8Jcitpbi:q:G:cjbp9UpmbedilvorzHOACXQLpPpkbbagaocdffa8JRb:q:W:cjbfhokalcdfhlaiczfgiaC6mbkkaohQaoTmoka8FaCfh8Fa8Ecefg8Ecl9hmbkdndndndnawTmbazaEcd4fRbbglciGPlbedwbkaCTmdaXaEfhlavaEfpbdbh8Kcbhoinalavcj;cbfaofpblbg8La8Aaofpblbg8MpmbzeHdOiAlCvXoQrLg8NaKaofpblbgyaYaofpblbg8PpmbzeHdOiAlCvXoQrLgIpmbezHdiOAlvCXorQLgacep9Taapxeeeeeeeeeeeeeeeeghp9op9Hp9rgaa8Kp9Ug8Kp9Abbbaladfgla8Kaaaapmlvorlvorlvorlvorp9Ug8Kp9Abbbaladfgla8KaaaapmwDqkwDqkwDqkwDqkp9Ug8Kp9Abbbaladfgla8KaaaapmxmPsxmPsxmPsxmPsp9Ug8Kp9Abbbaladfgla8Ka8NaIpmwDKYqk8AExm35Ps8E8Fgacep9Taaahp9op9Hp9rgap9Ug8Kp9Abbbaladfgla8Kaaaapmlvorlvorlvorlvorp9Ug8Kp9Abbbaladfgla8KaaaapmwDqkwDqkwDqkwDqkp9Ug8Kp9Abbbaladfgla8KaaaapmxmPsxmPsxmPsxmPsp9Ug8Kp9Abbbaladfgla8Ka8La8MpmwKDYq8AkEx3m5P8Es8Fg8Laya8PpmwKDYq8AkEx3m5P8Es8Fg8MpmbezHdiOAlvCXorQLgacep9Taaahp9op9Hp9rgap9Ug8Kp9Abbbaladfgla8Kaaaapmlvorlvorlvorlvorp9Ug8Kp9Abbbaladfgla8KaaaapmwDqkwDqkwDqkwDqkp9Ug8Kp9Abbbaladfgla8KaaaapmxmPsxmPsxmPsxmPsp9Ug8Kp9Abbbaladfgla8Ka8La8MpmwDKYqk8AExm35Ps8E8Fgacep9Taaahp9op9Hp9rgap9Ughp9Abbbaladfglahaaaapmlvorlvorlvorlvorp9Ughp9AbbbaladfglahaaaapmwDqkwDqkwDqkwDqkp9Ughp9AbbbaladfglahaaaapmxmPsxmPsxmPsxmPsp9Ug8Kp9AbbbaladfhlaoczfgoaC6mbxikkaCTmeaXaEfhlavaEfpbdbh8Kcbhoinalavcj;cbfaofpblbg8La8Aaofpblbg8MpmbzeHdOiAlCvXoQrLg8NaKaofpblbgyaYaofpblbg8PpmbzeHdOiAlCvXoQrLgIpmbezHdiOAlvCXorQLgacep:neaapxebebebebebebebebghp9op:bep9rgaa8Kp:oeg8Kp9Abbbaladfgla8Kaaaapmlvorlvorlvorlvorp:oeg8Kp9Abbbaladfgla8KaaaapmwDqkwDqkwDqkwDqkp:oeg8Kp9Abbbaladfgla8KaaaapmxmPsxmPsxmPsxmPsp:oeg8Kp9Abbbaladfgla8Ka8NaIpmwDKYqk8AExm35Ps8E8Fgacep:neaaahp9op:bep9rgap:oeg8Kp9Abbbaladfgla8Kaaaapmlvorlvorlvorlvorp:oeg8Kp9Abbbaladfgla8KaaaapmwDqkwDqkwDqkwDqkp:oeg8Kp9Abbbaladfgla8KaaaapmxmPsxmPsxmPsxmPsp:oeg8Kp9Abbbaladfgla8Ka8La8MpmwKDYq8AkEx3m5P8Es8Fg8Laya8PpmwKDYq8AkEx3m5P8Es8Fg8MpmbezHdiOAlvCXorQLgacep:neaaahp9op:bep9rgap:oeg8Kp9Abbbaladfgla8Kaaaapmlvorlvorlvorlvorp:oeg8Kp9Abbbaladfgla8KaaaapmwDqkwDqkwDqkwDqkp:oeg8Kp9Abbbaladfgla8KaaaapmxmPsxmPsxmPsxmPsp:oeg8Kp9Abbbaladfgla8Ka8La8MpmwDKYqk8AExm35Ps8E8Fgacep:neaaahp9op:bep9rgap:oeghp9Abbbaladfglahaaaapmlvorlvorlvorlvorp:oeghp9AbbbaladfglahaaaapmwDqkwDqkwDqkwDqkp:oeghp9AbbbaladfglahaaaapmxmPsxmPsxmPsxmPsp:oeg8Kp9AbbbaladfhlaoczfgoaC6mbxdkkaCTmbaXaEfhrcbhocbalcl4gl9Rc8FGhiavaEfpbdbhhinaravcj;cbfaofpblbg8Ka8Aaofpblbg8LpmbzeHdOiAlCvXoQrLg8MaKaofpblbg8NaYaofpblbgypmbzeHdOiAlCvXoQrLg8PpmbezHdiOAlvCXorQLgaaip:Reaaalp:Tep9qgaahp9rghp9Abbbaradfgrahaaaapmlvorlvorlvorlvorp9rghp9AbbbaradfgrahaaaapmwDqkwDqkwDqkwDqkp9rghp9AbbbaradfgrahaaaapmxmPsxmPsxmPsxmPsp9rghp9Abbbaradfgraha8Ma8PpmwDKYqk8AExm35Ps8E8Fgaaip:Reaaalp:Tep9qgap9rghp9Abbbaradfgrahaaaapmlvorlvorlvorlvorp9rghp9AbbbaradfgrahaaaapmwDqkwDqkwDqkwDqkp9rghp9AbbbaradfgrahaaaapmxmPsxmPsxmPsxmPsp9rghp9Abbbaradfgraha8Ka8LpmwKDYq8AkEx3m5P8Es8Fg8Ka8NaypmwKDYq8AkEx3m5P8Es8Fg8LpmbezHdiOAlvCXorQLgaaip:Reaaalp:Tep9qgap9rghp9Abbbaradfgrahaaaapmlvorlvorlvorlvorp9rghp9AbbbaradfgrahaaaapmwDqkwDqkwDqkwDqkp9rghp9AbbbaradfgrahaaaapmxmPsxmPsxmPsxmPsp9rghp9Abbbaradfgraha8Ka8LpmwDKYqk8AExm35Ps8E8Fgaaip:Reaaalp:Tep9qgap9rghp9Abbbaradfgrahaaaapmlvorlvorlvorlvorp9rghp9AbbbaradfgrahaaaapmwDqkwDqkwDqkwDqkp9rghp9AbbbaradfgrahaaaapmxmPsxmPsxmPsxmPsp9rghp9AbbbaradfhraoczfgoaC6mbkkaEclfgEad6mbkdnaXavcjdf9hmbaAad2goTmbaOavcjdfao;8qbbkdnammbavaXaAcufad2fad;8qbbkaAaHfhHc9:hoaQhPaQmbxlkkaeTmbaDalfhrcbhocuhlinaralaD9RglfaD6mdasaeao9Raoasfae6Eaofgoae6mbkaial9RhPkcbc99axaP9RakSEhoxekc9:hokavcj;kbf8Kjjjjbaokwbz:bjjjbkpPesu8Jjjjjbc;ae9Rgv8Kjjjjbc9:hodnalaeci9UgrcHf6mbcuhoaiRbbgwc;WeGc;Ge9hmbawcsGgwce0mbavc;abfcFecje;8kbav9cu83iUav9cu83i8Wav9cu83iyav9cu83iaav9cu83iKav9cu83izav9cu83iwav9cu83ibaialfc9WfhDaicefgiarfhqdndnaeci9pmbaqhexekcmcsawceSEhkcbhxcbhmaqhecbhlcbhoindndnaiRbbgrc;Ve0mbavc;abfaoarcu7gPcl4fcsGcitfgwydlhsawydbhzdndnarcsGgwak9pmbavalaPfcsGcdtfydbaxawEhraxawTgHfhxxekdnaeaD9nmbc9:hoxokdndnawcsSmbcehHamawcetfcWfhrxekaecefhrae8SbbgwcFeGhPdndnawcu9mmbarhexekaecvfheaPcFbGhPcrhwdninar8SbbgHcFbGawtaPVhPaHcu9kmearcefhrawcrfgwc8J9hmbxdkkarcefhekcehHaPce4cbaPceG9R7amfhrkarhmkavc;abfaocitfgwarBdbawasBdlavalcdtfarBdbavc;abfaocefcsGcitfgwazBdbawarBdlaocdfhoaHalfhldnadcd9hmbabar87elabas87edabaz87ebabcofhbxdkabarBdwabasBdlabazBdbabcxfhbxekdnarcpe0mbavalaDarcsGfRbbgwcl4gP9RcsGcdtfydbaxcefgsaPEhravalaw9RcsGcdtfydbasaPTgzfgsawcsGgPEhwaPThPdndnadcd9hmbabaw87elabar87edabax87ebcohHxekabawBdwabarBdlabaxBdbcxhHkavalcdtfaxBdbavc;abfaocitfgOarBdbaOaxBdlavalcefglcsGcdtfarBdbavc;abfaocefcsGcitfgOawBdbaOarBdlavalazfglcsGcdtfawBdbavc;abfaocdfcsGcitfgraxBdbarawBdlaocifhoabaHfhbalaPfhlasaPfhxxekdnaeaD9nmbc9:hoxlkaxcbaeRbbgwEgHarc;:eSgrfhsawcsGhOdndnawcl4gAmbascefhzxekashzavalaA9RcsGcdtfydbhskdndnaOmbazcefhxxekazhxavalaw9RcsGcdtfydbhzkdndnarTmbaecefhrxekaecdfhrae8SbegPcFeGhwdnaPcu9kmbaecofhHawcFbGhwcrhedninar8SbbgPcFbGaetawVhwaPcu9kmearcefhraecrfgec8J9hmbkaHhrxekarcefhrkawce4cbawceG9R7amfgmhHkdndnaAcsSmbarhwxekarcefhwar8SbbgecFeGhPdnaecu9kmbarcvfhsaPcFbGhPcrhedninaw8SbbgrcFbGaetaPVhParcu9kmeawcefhwaecrfgec8J9hmbkashwxekawcefhwkaPce4cbaPceG9R7amfgmhskdndnaOcsSmbawhexekawcefheaw8SbbgrcFeGhPdnarcu9kmbawcvfhzaPcFbGhPcrhrdninae8SbbgwcFbGartaPVhPawcu9kmeaecefhearcrfgrc8J9hmbkazhexekaecefhekaPce4cbaPceG9R7amfgmhzkdndnadcd9hmbabaz87elabas87edabaH87ebcohrxekabazBdwabasBdlabaHBdbcxhrkavc;abfaocitfgwasBdbawaHBdlavalcdtfaHBdbavc;abfaocefcsGcitfgwazBdbawasBdlavalcefglcsGcdtfasBdbavc;abfaocdfcsGcitfgwaHBdbawazBdlavalaATaAcsSVfglcsGcdtfazBdbalaOTaOcsSVfhlaocifhoabarfhbkaocsGhoalcsGhlaicefgiaq6mbkkcbc99aeaDSEhokavc;aef8Kjjjjbaok:clevu8Jjjjjbcz9Rhvdnalaecvf9pmbc9:skdnaiRbbc;:eGc;qeSmbcuskav9cb83iwaicefhoaialfc98fhrdnaeTmbdnadcdSmbcbhwindnaoar6mbc9:skaocefhlao8SbbgicFeGhddndnaicu9mmbalhoxekaocvfhoadcFbGhdcrhidninal8SbbgDcFbGaitadVhdaDcu9kmealcefhlaicrfgic8J9hmbxdkkalcefhokabawcdtfadc8Etc8F91adcd47avcwfadceGcdtVglydbfgiBdbalaiBdbawcefgwae9hmbxdkkcbhwindnaoar6mbc9:skaocefhlao8SbbgicFeGhddndnaicu9mmbalhoxekaocvfhoadcFbGhdcrhidninal8SbbgDcFbGaitadVhdaDcu9kmealcefhlaicrfgic8J9hmbxdkkalcefhokabawcetfadc8Etc8F91adcd47avcwfadceGcdtVglydbfgi87ebalaiBdbawcefgwae9hmbkkcbc99aoarSEk;Toio97eue97aec98Ghedndnadcl9hmbaeTmecbhdinababpbbbgicKp:RecKp:Sep;6eglaicwp:RecKp:Sep;6ealp;Geaiczp:RecKp:Sep;6egvp;Gep;Kep;Legopxbbbbbbbbbbbbbbbbp:2egralpxbbbjbbbjbbbjbbbjgwp9op9rp;Keglpxbb;:9cbb;:9cbb;:9cbb;:9calalp;Meaoaop;Meavaravawp9op9rp;Keglalp;Mep;Kep;Kep;Jep;Negvp;Mepxbbn0bbn0bbn0bbn0grp;KepxFbbbFbbbFbbbFbbbp9oaipxbbbFbbbFbbbFbbbFp9op9qalavp;Mearp;Kecwp:RepxbFbbbFbbbFbbbFbbp9op9qaoavp;Mearp;Keczp:RepxbbFbbbFbbbFbbbFbp9op9qpkbbabczfhbadclfgdae6mbxdkkaeTmbcbhdinabczfgDaDpbbbgipxbbbbbbFFbbbbbbFFgwp9oabpbbbgoaipmbediwDqkzHOAKY8AEgvczp:Reczp:Sep;6eglaoaipmlvorxmPsCXQL358E8FpxFubbFubbFubbFubbp9op;6eavczp:Sep;6egvp;Gealp;Gep;Kep;Legipxbbbbbbbbbbbbbbbbp:2egralpxbbbjbbbjbbbjbbbjgqp9op9rp;Keglpxb;:FSb;:FSb;:FSb;:FSalalp;Meaiaip;Meavaravaqp9op9rp;Keglalp;Mep;Kep;Kep;Jep;Negvp;Mepxbbn0bbn0bbn0bbn0grp;KepxFFbbFFbbFFbbFFbbp9oaiavp;Mearp;Keczp:Rep9qgialavp;Mearp;KepxFFbbFFbbFFbbFFbbp9oglpmwDKYqk8AExm35Ps8E8Fp9qpkbbabaoawp9oaialpmbezHdiOAlvCXorQLp9qpkbbabcafhbadclfgdae6mbkkk;2ileue97euo97dnaec98GgiTmbcbheinabcKfpx:ji:1S:ji:1S:ji:1S:ji:1SabpbbbglabczfgvpbbbgopmlvorxmPsCXQL358E8Fgrczp:Segwpxibbbibbbibbbibbbp9qp;6egDp;NegqaDaDp;MegDaDp;KealaopmbediwDqkzHOAKY8AEgDczp:Reczp:Sep;6eglalp;MeaDczp:Sep;6egoaop;Mearczp:Reczp:Sep;6egrarp;Mep;Kep;Kep;Lepxbbbbbbbbbbbbbbbbp:4ep;Jep;Mepxbbn0bbn0bbn0bbn0gDp;KepxFFbbFFbbFFbbFFbbgkp9oaqaop;MeaDp;Keczp:Rep9qgoaqalp;MeaDp;Keakp9oaqarp;MeaDp;Keczp:Rep9qgDpmwDKYqk8AExm35Ps8E8Fglp5eawclp:RegqpEi:T:j83ibavalp5baqpEd:T:j83ibabcwfaoaDpmbezHdiOAlvCXorQLgDp5eaqpEe:T:j83ibabaDp5baqpEb:T:j83ibabcafhbaeclfgeai6mbkkkuee97dnadcd4ae2c98GgeTmbcbhdinababpbbbgicwp:Recwp:Sep;6eaicep:SepxbbjFbbjFbbjFbbjFp9opxbbjZbbjZbbjZbbjZp:Uep;Mepkbbabczfhbadclfgdae6mbkkk:Sodw97euaec98Ghedndnadcl9hmbaeTmecbhdinabpxbbuJbbuJbbuJbbuJabpbbbgicKp:TeglaicYp:Tep9qgvcdp:Teavp9qgvclp:Teavp9qgop;6ep;Negvaicwp:RecKp:SegraipxFbbbFbbbFbbbFbbbgwp9ogDp:Uep;6ep;Mepxbbn0bbn0bbn0bbn0gqp;Kecwp:RepxbFbbbFbbbFbbbFbbp9oavaDarp:Xeaiczp:RecKp:Segip:Uep;6ep;Meaqp;Keawp9op9qavaDaraip:Uep:Xep;6ep;Meaqp;Keczp:RepxbbFbbbFbbbFbbbFbp9op9qavaoalcep:Rep9oalpxebbbebbbebbbebbbp9op9qp;6ep;Meaqp;KecKp:Rep9qpkbbabczfhbadclfgdae6mbxdkkaeTmbcbhdinabczfgkpxbFu9hbFu9hbFu9hbFu9habpbbbglakpbbbgrpmlvorxmPsCXQL358E8Fgvczp:TegqavcHp:Tep9qgicdp:Teaip9qgiclp:Teaip9qgicwp:Teaip9qgop;6ep;NegialarpmbediwDqkzHOAKY8AEgDpxFFbbFFbbFFbbFFbbglp9ograDczp:Segwp:Ueavczp:Reczp:SegDp:Xep;6ep;Mepxbbn0bbn0bbn0bbn0gvp;Kealp9oaiarawaDp:Uep:Xep;6ep;Meavp;Keczp:Rep9qgwaiaoaqcep:Rep9oaqpxebbbebbbebbbebbbp9op9qp;6ep;Meavp;Keczp:ReaiaDarp:Uep;6ep;Meavp;Kealp9op9qgipmwDKYqk8AExm35Ps8E8FpkbbabawaipmbezHdiOAlvCXorQLpkbbabcafhbadclfgdae6mbkkk9teiucbcbydj:G:cjbgeabcifc98GfgbBdj:G:cjbdndnabZbcztgd9nmbcuhiabad9RcFFifcz4nbcuSmekaehikaikkxebcj:Gdklz:zbb`,n=new Uint8Array([0,97,115,109,1,0,0,0,1,4,1,96,0,0,3,3,2,0,0,5,3,1,0,1,12,1,0,10,22,2,12,0,65,0,65,0,65,0,252,10,0,0,11,7,0,65,0,253,15,26,11]),r=new Uint8Array([32,0,65,2,1,106,34,33,3,128,11,4,13,64,6,253,10,7,15,116,127,5,8,12,40,16,19,54,20,9,27,255,113,17,42,67,24,23,146,148,18,14,22,45,70,69,56,114,101,21,25,63,75,136,108,28,118,29,73,115]);if(typeof WebAssembly!=`object`)return{supported:!1};var i=WebAssembly.validate(n)?s(t):s(e),a,o=WebAssembly.instantiate(i,{}).then(function(e){a=e.instance,a.exports.__wasm_call_ctors()});function s(e){for(var t=new Uint8Array(e.length),n=0;n<e.length;++n){var i=e.charCodeAt(n);t[n]=i>96?i-97:i>64?i-39:i+4}for(var a=0,n=0;n<e.length;++n)t[a++]=t[n]<60?r[t[n]]:(t[n]-60)*64+t[++n];return t.buffer.slice(0,a)}function c(e,t,n,r,i,a,o){var s=e.exports.sbrk,c=r+3&-4,l=s(c*i),u=s(a.length),d=new Uint8Array(e.exports.memory.buffer);d.set(a,u);var f=t(l,r,i,u,a.length);if(f==0&&o&&o(l,c,i),n.set(d.subarray(l,l+r*i)),s(l-s(0)),f!=0)throw Error(`Malformed buffer data: `+f)}var l={NONE:``,OCTAHEDRAL:`meshopt_decodeFilterOct`,QUATERNION:`meshopt_decodeFilterQuat`,EXPONENTIAL:`meshopt_decodeFilterExp`,COLOR:`meshopt_decodeFilterColor`},u={ATTRIBUTES:`meshopt_decodeVertexBuffer`,TRIANGLES:`meshopt_decodeIndexBuffer`,INDICES:`meshopt_decodeIndexSequence`},d=[],f=0;function p(e){var t={object:new Worker(e),pending:0,requests:{}};return t.object.onmessage=function(e){var n=e.data;t.pending-=n.count,t.requests[n.id][n.action](n.value),delete t.requests[n.id]},t}function m(e){for(var t=`self.ready = WebAssembly.instantiate(new Uint8Array([`+new Uint8Array(i)+`]), {}).then(function(result) { result.instance.exports.__wasm_call_ctors(); return result.instance; });self.onmessage = `+g.name+`;`+c.toString()+g.toString(),n=new Blob([t],{type:`text/javascript`}),r=URL.createObjectURL(n),a=d.length;a<e;++a)d[a]=p(r);for(var a=e;a<d.length;++a)d[a].object.postMessage({});d.length=e,URL.revokeObjectURL(r)}function h(e,t,n,r,i){for(var a=d[0],o=1;o<d.length;++o)d[o].pending<a.pending&&(a=d[o]);return new Promise(function(o,s){var c=new Uint8Array(n),l=++f;a.pending+=e,a.requests[l]={resolve:o,reject:s},a.object.postMessage({id:l,count:e,size:t,source:c,mode:r,filter:i},[c.buffer])})}function g(e){var t=e.data;self.ready.then(function(e){if(!t.id)return self.close();try{var n=new Uint8Array(t.count*t.size);c(e,e.exports[t.mode],n,t.count,t.size,t.source,e.exports[t.filter]),self.postMessage({id:t.id,count:t.count,action:`resolve`,value:n},[n.buffer])}catch(e){self.postMessage({id:t.id,count:t.count,action:`reject`,value:e})}})}return{ready:o,supported:!0,useWorkers:function(e){m(e)},decodeVertexBuffer:function(e,t,n,r,i){c(a,a.exports.meshopt_decodeVertexBuffer,e,t,n,r,a.exports[l[i]])},decodeIndexBuffer:function(e,t,n,r){c(a,a.exports.meshopt_decodeIndexBuffer,e,t,n,r)},decodeIndexSequence:function(e,t,n,r){c(a,a.exports.meshopt_decodeIndexSequence,e,t,n,r)},decodeGltfBuffer:function(e,t,n,r,i,o){c(a,a.exports[u[i]],e,t,n,r,a.exports[l[o]])},decodeGltfBufferAsync:function(e,t,n,r,i){return d.length>0?h(e,t,n,u[r],l[i]):o.then(function(){var o=new Uint8Array(e*t);return c(a,a.exports[u[r]],o,e,t,n,a.exports[l[i]]),o})}}})();function nD(e){return typeof e==`string`?e:[`NONE`,`OCTAHEDRAL`,`QUATERNION`,`EXPONENTIAL`,`COLOR`][e]}async function rD(e,t,n,r,i,a=`NONE`){await tD.ready,tD.decodeGltfBuffer(e,t,n,r,i,nD(a))}async function iD(e,t,n){if(!t.gltf?.decompressMeshes||!t.gltf.loadBuffers)return;aD(e.json);let r=new q(e),i=e.json.bufferViews||[],a=i.map(e=>oD(r,e,n));await Promise.all(a);for(let e of i)r.removeObjectExtension(e,n);for(let t of e.json.buffers||[])r.removeObjectExtension(t,n);r.removeExtension(n)}function aD(e){let t=e.bufferViews||[];for(let e=0;e<t.length;e++){let n=t[e].extensions;if(n?.KHR_meshopt_compression&&n.EXT_meshopt_compression)throw Error(`glTF bufferView ${e} cannot use both KHR_meshopt_compression and EXT_meshopt_compression.`)}let n=e.buffers||[];for(let e=0;e<n.length;e++){let t=n[e].extensions;if(t?.KHR_meshopt_compression&&t.EXT_meshopt_compression)throw Error(`glTF buffer ${e} cannot use both KHR_meshopt_compression and EXT_meshopt_compression.`)}}async function oD(e,t,n){let r=e.getObjectExtension(t,n);if(!r)return;let{byteOffset:i=0,byteLength:a,byteStride:o,count:s,mode:c,filter:l=`NONE`,buffer:u}=r,d=e.gltf.buffers[u],f=e.gltf.buffers[t.buffer],p=new Uint8Array(d.arrayBuffer,d.byteOffset+i,a);await rD(new Uint8Array(f.arrayBuffer,f.byteOffset+(t.byteOffset||0),t.byteLength),s,o,p,c,l)}var sD=e({decode:()=>lD,name:()=>cD}),cD=`EXT_meshopt_compression`;async function lD(e,t){await iD(e,t,cD)}var uD=e({decode:()=>fD,name:()=>dD}),dD=`KHR_meshopt_compression`;async function fD(e,t){await iD(e,t,dD)}var pD=e({name:()=>hD,preprocess:()=>gD}),mD=`EXT_texture_webp`,hD=mD;function gD(e,t){let n=new q(e);if(!LC(`image/webp`)){if(n.getRequiredExtensions().includes(mD))throw Error(`gltf: Required extension ${mD} not supported by browser`);return}let{json:r}=n;for(let e of r.textures||[]){let t=n.getObjectExtension(e,mD);t&&(e.source=t.source),n.removeObjectExtension(e,mD)}n.removeExtension(mD)}var _D=e({name:()=>yD,preprocess:()=>bD}),vD=`EXT_texture_avif`,yD=vD;async function bD(e,t){let n=new q(e);if(!(await PC()).has(`image/avif`)){if(n.getRequiredExtensions().includes(vD))throw Error(`gltf: Required extension ${vD} not supported by browser`);return}let{json:r}=n;for(let e of r.textures||[]){let t=n.getObjectExtension(e,vD);t&&(e.source=t.source),n.removeObjectExtension(e,vD)}n.removeExtension(vD)}var xD=e({name:()=>CD,preprocess:()=>wD}),SD=`KHR_texture_basisu`,CD=SD;function wD(e,t){let n=new q(e),{json:r}=n;for(let e of r.textures||[]){let t=n.getObjectExtension(e,SD);t&&(e.source=t.source,n.removeObjectExtension(e,SD))}n.removeExtension(SD)}function TD(){return globalThis._loadersgl_?.version||(globalThis._loadersgl_=globalThis._loadersgl_||{},globalThis._loadersgl_.version=`4.5.4`),globalThis._loadersgl_.version}var ED=TD();function DD(e,t){if(!e)throw Error(t||`loaders.gl assertion failed.`)}var OD={self:typeof self<`u`&&self,window:typeof window<`u`&&window,global:typeof global<`u`&&global,document:typeof document<`u`&&document};OD.self||OD.window||OD.global,OD.window||OD.self||OD.global,OD.global||OD.self||OD.window,OD.document;var kD=typeof process!=`object`||String(process)!==`[object process]`||process.browser,AD=typeof importScripts==`function`;typeof window<`u`&&window.orientation;var jD=typeof process<`u`&&process.version&&/v([0-9]*)/.exec(process.version);jD&&parseFloat(jD[1]);var MD={};function ND(e={}){let t=e.useLocalLibraries??e.core?.useLocalLibraries,n=e.CDN??e.core?.CDN,r=e.modules;return{...t===void 0?{}:{useLocalLibraries:t},...n===void 0?{}:{CDN:n},...r===void 0?{}:{modules:r}}}async function PD(e,t=null,n={},r=null){if(t){let i=n.modules?.[r||e];if(i!==void 0&&typeof i!=`string`)return i;e=FD(e,t,n,r)}return MD[e]=MD[e]||ID(e),await MD[e]}function FD(e,t,n={},r=null){if(n?.core)throw Error(`loadLibrary: options.core must be pre-normalized`);r||=e;let i=n.modules||{};return typeof i[r]==`string`?i[r]:!n.useLocalLibraries&&e.startsWith(`http`)?e:kD?n.CDN?(DD(n.CDN.startsWith(`http`)),`${n.CDN}/${t}@${ED}/dist/libs/${r}`):AD?`../src/libs/${r}`:`modules/${t}/src/libs/${r}`:`modules/${t}/dist/libs/${r}`}async function ID(e){if(e.endsWith(`wasm`))return await RD(e);if(!kD){let{requireFromFile:t}=globalThis.loaders||{};try{let n=await t?.(e);return n||!e.includes(`/dist/libs/`)?n:await t?.(e.replace(`/dist/libs/`,`/src/libs/`))}catch(n){if(e.includes(`/dist/libs/`))try{return await t?.(e.replace(`/dist/libs/`,`/src/libs/`))}catch{}return console.error(n),null}}return AD?importScripts(e):LD(await zD(e),e)}function LD(e,t){if(!kD){let{requireFromString:n}=globalThis.loaders||{};return n?.(e,t)}if(AD)return eval.call(globalThis,e),null;let n=document.createElement(`script`);n.id=t;try{n.appendChild(document.createTextNode(e))}catch{n.text=e}return document.body.appendChild(n),null}async function RD(e){let{readFileAsArrayBuffer:t}=globalThis.loaders||{};if(kD||!t||e.startsWith(`http`))return await(await fetch(e)).arrayBuffer();try{return await t(e)}catch{if(e.includes(`/dist/libs/`))return await t(e.replace(`/dist/libs/`,`/src/libs/`));throw Error(`Failed to load ArrayBuffer from ${e}`)}}async function zD(e){let{readFileAsText:t}=globalThis.loaders||{};if(kD||!t||e.startsWith(`http`))return await(await fetch(e)).text();try{return await t(e)}catch{if(e.includes(`/dist/libs/`))return await t(e.replace(`/dist/libs/`,`/src/libs/`));throw Error(`Failed to load text from ${e}`)}}function BD(e,t){return new Promise((n,r)=>{let i=!1;function a(e){i||(i=!0,r(e))}function o(e){if(!i){i=!0;try{n(t(e))}catch(e){r(e)}}}try{e(o,a)?.then(o,a)}catch(e){a(e)}})}var VD=`1.5.6`,HD=`1.4.1`,UD=`https://www.gstatic.com/draco/versioned/decoders/${VD}`,WD={DECODER:`draco_wasm_wrapper.js`,DECODER_WASM:`draco_decoder.wasm`,FALLBACK_DECODER:`draco_decoder.js`,ENCODER:`draco_encoder.js`},GD={[WD.DECODER]:`${UD}/${WD.DECODER}`,[WD.DECODER_WASM]:`${UD}/${WD.DECODER_WASM}`,[WD.FALLBACK_DECODER]:`${UD}/${WD.FALLBACK_DECODER}`,[WD.ENCODER]:`https://raw.githubusercontent.com/google/draco/${HD}/javascript/${WD.ENCODER}`},KD;async function qD(e={},t){let n=e.modules||{};return n.draco3d?KD||=BD(()=>n.draco3d.createDecoderModule({}),e=>({draco:e})):KD||=YD(e,t),await KD}function JD(e,t){if(e&&typeof e==`object`){if(e.default)return e.default;if(e[t])return e[t]}return e}async function YD(e,t){let n,r;switch(t){case`js`:n=await PD(GD[WD.FALLBACK_DECODER],`draco`,e,WD.FALLBACK_DECODER);break;default:try{[n,r]=await Promise.all([await PD(GD[WD.DECODER],`draco`,e,WD.DECODER),await PD(GD[WD.DECODER_WASM],`draco`,e,WD.DECODER_WASM)])}catch{n=null,r=null}}return n=JD(n,`DracoDecoderModule`),n||=globalThis.DracoDecoderModule,!n&&!kD&&([n,r]=await Promise.all([await PD(GD[WD.DECODER],`draco`,{...e,useLocalLibraries:!0},WD.DECODER),await PD(GD[WD.DECODER_WASM],`draco`,{...e,useLocalLibraries:!0},WD.DECODER_WASM)]),n=JD(n,`DracoDecoderModule`),n||=globalThis.DracoDecoderModule),await XD(n,r)}function XD(e,t){if(typeof e!=`function`)throw Error(`DracoDecoderModule could not be loaded`);let n={};return t&&(n.wasmBinary=t),BD(t=>e({...n,onModuleLoaded:t}),e=>({draco:e}))}var ZD=`4.5.4`;function QD(e){switch(e.constructor){case Int8Array:return`int8`;case Uint8Array:case Uint8ClampedArray:return`uint8`;case Int16Array:return`int16`;case Uint16Array:return`uint16`;case Int32Array:return`int32`;case Uint32Array:return`uint32`;case Float32Array:return`float32`;case Float64Array:return`float64`;default:return`null`}}function $D(e){let t=1/0,n=1/0,r=1/0,i=-1/0,a=-1/0,o=-1/0,s=e.POSITION?e.POSITION.value:[],c=s&&s.length;for(let e=0;e<c;e+=3){let c=s[e],l=s[e+1],u=s[e+2];t=c<t?c:t,n=l<n?l:n,r=u<r?u:r,i=c>i?c:i,a=l>a?l:a,o=u>o?u:o}return[[t,n,r],[i,a,o]]}function eO(e,t,n){let r=QD(t.value),i=n||tO(t);return{name:e,type:{type:`fixed-size-list`,listSize:t.size,children:[{name:`value`,type:r}]},nullable:!1,metadata:i}}function tO(e){let t={};return`byteOffset`in e&&(t.byteOffset=e.byteOffset.toString(10)),`byteStride`in e&&(t.byteStride=e.byteStride.toString(10)),`normalized`in e&&(t.normalized=e.normalized.toString()),t}function nO(e,t,n){let r=aO(t.metadata),i=[],a=rO(t.attributes);for(let t in e){let n=e[t],r=iO(t,n,a[t]);i.push(r)}if(n){let e=iO(`indices`,n);i.push(e)}return{fields:i,metadata:r}}function rO(e){let t={};for(let n in e){let r=e[n];t[r.name||`undefined`]=r}return t}function iO(e,t,n){return eO(e,t,n?aO(n.metadata):void 0)}function aO(e){Object.entries(e);let t={};for(let n in e)t[`${n}.string`]=JSON.stringify(e[n]);return t}var oO={POSITION:`POSITION`,NORMAL:`NORMAL`,COLOR:`COLOR_0`,TEX_COORD:`TEXCOORD_0`},sO={1:Int8Array,2:Uint8Array,3:Int16Array,4:Uint16Array,5:Int32Array,6:Uint32Array,9:Float32Array},cO=4,lO=class{draco;decoder;metadataQuerier;constructor(e){this.draco=e,this.decoder=new this.draco.Decoder,this.metadataQuerier=new this.draco.MetadataQuerier}destroy(){this.draco.destroy(this.decoder),this.draco.destroy(this.metadataQuerier)}parseSync(e,t={}){let n=new this.draco.DecoderBuffer;n.Init(new Int8Array(e),e.byteLength),this._disableAttributeTransforms(t);let r=this.decoder.GetEncodedGeometryType(n),i=r===this.draco.TRIANGULAR_MESH?new this.draco.Mesh:new this.draco.PointCloud;try{let e;switch(r){case this.draco.TRIANGULAR_MESH:e=this.decoder.DecodeBufferToMesh(n,i);break;case this.draco.POINT_CLOUD:e=this.decoder.DecodeBufferToPointCloud(n,i);break;default:throw Error(`DRACO: Unknown geometry type.`)}if(!e.ok()||!i.ptr){let t=`DRACO decompression failed: ${e.error_msg()}`;throw Error(t)}let a=this._getDracoLoaderData(i,r,t),o=this._getMeshData(i,a,t),s=$D(o.attributes),c=nO(o.attributes,a,o.indices);return{loader:`draco`,loaderData:a,header:{vertexCount:i.num_points(),boundingBox:s},...o,schema:c}}finally{this.draco.destroy(n),i&&this.draco.destroy(i)}}_getDracoLoaderData(e,t,n){let r=this._getTopLevelMetadata(e),i=this._getDracoAttributes(e,n);return{geometry_type:t,num_attributes:e.num_attributes(),num_points:e.num_points(),num_faces:e instanceof this.draco.Mesh?e.num_faces():0,metadata:r,attributes:i}}_getDracoAttributes(e,t){let n={};for(let r=0;r<e.num_attributes();r++){let i=this.decoder.GetAttribute(e,r),a=this._getAttributeMetadata(e,r);n[i.unique_id()]={unique_id:i.unique_id(),attribute_type:i.attribute_type(),data_type:i.data_type(),num_components:i.num_components(),byte_offset:i.byte_offset(),byte_stride:i.byte_stride(),normalized:i.normalized(),attribute_index:r,metadata:a};let o=this._getQuantizationTransform(i,t);o&&(n[i.unique_id()].quantization_transform=o);let s=this._getOctahedronTransform(i,t);s&&(n[i.unique_id()].octahedron_transform=s)}return n}_getMeshData(e,t,n){let r=this._getMeshAttributes(t,e,n);if(!r.POSITION)throw Error(`DRACO: No position attribute found.`);if(e instanceof this.draco.Mesh)switch(n.topology){case`triangle-strip`:return{topology:`triangle-strip`,mode:4,attributes:r,indices:{value:this._getTriangleStripIndices(e),size:1}};default:return{topology:`triangle-list`,mode:5,attributes:r,indices:{value:this._getTriangleListIndices(e),size:1}}}return{topology:`point-list`,mode:0,attributes:r}}_getMeshAttributes(e,t,n){let r={};for(let i of Object.values(e.attributes)){let e=this._deduceAttributeName(i,n);i.name=e;let a=this._getAttributeValues(t,i);if(a){let{value:t,size:n}=a;r[e]={value:t,size:n,byteOffset:i.byte_offset,byteStride:i.byte_stride,normalized:i.normalized}}}return r}_getTriangleListIndices(e){let t=e.num_faces()*3,n=t*cO,r=this.draco._malloc(n);try{return this.decoder.GetTrianglesUInt32Array(e,n,r),new Uint32Array(this.draco.HEAPF32.buffer,r,t).slice()}finally{this.draco._free(r)}}_getTriangleStripIndices(e){let t=new this.draco.DracoInt32Array;try{return this.decoder.GetTriangleStripsFromMesh(e,t),fO(t)}finally{this.draco.destroy(t)}}_getAttributeValues(e,t){let n=sO[t.data_type];if(!n)return console.warn(`DRACO: Unsupported attribute type ${t.data_type}`),null;let r=t.num_components,i=e.num_points()*r,a=i*n.BYTES_PER_ELEMENT,o=uO(this.draco,n),s,c=this.draco._malloc(a);try{let r=this.decoder.GetAttribute(e,t.attribute_index);this.decoder.GetAttributeDataArrayForAllPoints(e,r,o,a,c),s=new n(this.draco.HEAPF32.buffer,c,i).slice()}finally{this.draco._free(c)}return{value:s,size:r}}_deduceAttributeName(e,t){let n=e.unique_id;for(let[e,r]of Object.entries(t.extraAttributes||{}))if(r===n)return e;let r=e.attribute_type;for(let e in oO)if(this.draco[e]===r)return oO[e];let i=t.attributeNameEntry||`name`;return e.metadata[i]?e.metadata[i].string:`CUSTOM_ATTRIBUTE_${n}`}_getTopLevelMetadata(e){let t=this.decoder.GetMetadata(e);return this._getDracoMetadata(t)}_getAttributeMetadata(e,t){let n=this.decoder.GetAttributeMetadata(e,t);return this._getDracoMetadata(n)}_getDracoMetadata(e){if(!e||!e.ptr)return{};let t={},n=this.metadataQuerier.NumEntries(e);for(let r=0;r<n;r++){let n=this.metadataQuerier.GetEntryName(e,r);t[n]=this._getDracoMetadataField(e,n)}return t}_getDracoMetadataField(e,t){let n=new this.draco.DracoInt32Array;try{this.metadataQuerier.GetIntEntryArray(e,t,n);let r=dO(n);return{int:this.metadataQuerier.GetIntEntry(e,t),string:this.metadataQuerier.GetStringEntry(e,t),double:this.metadataQuerier.GetDoubleEntry(e,t),intArray:r}}finally{this.draco.destroy(n)}}_disableAttributeTransforms(e){let{quantizedAttributes:t=[],octahedronAttributes:n=[]}=e,r=[...t,...n];for(let e of r)this.decoder.SkipAttributeTransform(this.draco[e])}_getQuantizationTransform(e,t){let{quantizedAttributes:n=[]}=t,r=e.attribute_type();if(n.map(e=>this.decoder[e]).includes(r)){let t=new this.draco.AttributeQuantizationTransform;try{if(t.InitFromAttribute(e))return{quantization_bits:t.quantization_bits(),range:t.range(),min_values:new Float32Array([1,2,3]).map(e=>t.min_value(e))}}finally{this.draco.destroy(t)}}return null}_getOctahedronTransform(e,t){let{octahedronAttributes:n=[]}=t,r=e.attribute_type();if(n.map(e=>this.decoder[e]).includes(r)){let t=new this.draco.AttributeQuantizationTransform;try{if(t.InitFromAttribute(e))return{quantization_bits:t.quantization_bits()}}finally{this.draco.destroy(t)}}return null}};function uO(e,t){switch(t){case Float32Array:return e.DT_FLOAT32;case Int8Array:return e.DT_INT8;case Int16Array:return e.DT_INT16;case Int32Array:return e.DT_INT32;case Uint8Array:return e.DT_UINT8;case Uint16Array:return e.DT_UINT16;case Uint32Array:return e.DT_UINT32;default:return e.DT_INVALID}}function dO(e){let t=e.size(),n=new Int32Array(t);for(let r=0;r<t;r++)n[r]=e.GetValue(r);return n}function fO(e){let t=e.size(),n=new Int32Array(t);for(let r=0;r<t;r++)n[r]=e.GetValue(r);return n}var pO={dataType:null,batchType:null,name:`Draco`,id:`draco`,module:`draco`,version:ZD,worker:!0,extensions:[`drc`],mimeTypes:[`application/octet-stream`],binary:!0,tests:[`DRACO`],options:{draco:{decoderType:typeof WebAssembly==`object`?`wasm`:`js`,extraAttributes:{},attributeNameEntry:void 0}},parse:mO};async function mO(e,t){let{draco:n}=await qD(ND(t),t?.draco?.decoderType||`wasm`),r=new lO(n);try{return r.parseSync(e,t?.draco)}finally{r.destroy()}}function hO(e){let t={};for(let n in e){let r=e[n];n!==`indices`&&(t[n]=gO(r))}return t}function gO(e){let{buffer:t,size:n,count:r}=_O(e);return{value:t,size:n,byteOffset:0,count:r,type:$C(n),componentType:ew(t)}}function _O(e){let t=e,n=1,r=0;return e&&e.value&&(t=e.value,n=e.size||1),t&&(ArrayBuffer.isView(t)||(t=vO(t,Float32Array)),r=t.length/n),{buffer:t,size:n,count:r}}function vO(e,t,n=!1){return e?Array.isArray(e)||n&&!(e instanceof t)?new t(e):e:null}var yO=e({decode:()=>CO,encode:()=>wO,name:()=>xO,preprocess:()=>SO}),bO=`KHR_draco_mesh_compression`,xO=bO;function SO(e,t,n){let r=new q(e);for(let e of OO(r))r.getObjectExtension(e,bO)}async function CO(e,t,n){if(!t?.gltf?.decompressMeshes)return;let r=new q(e),i=[];for(let e of OO(r))r.getObjectExtension(e,bO)&&i.push(TO(r,e,t,n));await Promise.all(i),r.removeExtension(bO)}function wO(e,t={}){let n=new q(e);for(let e of n.json.meshes||[])EO(e,t),n.addRequiredExtension(bO)}async function TO(e,t,n,r){let i=e.getObjectExtension(t,bO);if(!i)return;let a=e.getTypedArrayForBufferView(i.bufferView),o=HS(a.buffer,a.byteOffset),s={...n};delete s[`3d-tiles`];let c=await NS(o,pO,s,r),l=hO(c.attributes);for(let[n,r]of Object.entries(l))if(n in t.attributes){let i=t.attributes[n],a=e.getAccessor(i);a?.min&&a?.max&&(r.min=a.min,r.max=a.max)}t.attributes=l,c.indices&&(t.indices=gO(c.indices)),e.removeObjectExtension(t,bO),DO(t)}function EO(e,t,n=4,r,i){if(!r.DracoWriter)throw Error(`options.gltf.DracoWriter not provided`);let a=r.DracoWriter.encodeSync({attributes:e}),o=i?.parseSync?.({attributes:e}),s=r._addFauxAttributes(o.attributes),c=r.addBufferView(a);return{primitives:[{attributes:s,mode:n,extensions:{[bO]:{bufferView:c,attributes:s}}}]}}function DO(e){if(!e.attributes&&Object.keys(e.attributes).length>0)throw Error(`glTF: Empty primitive detected: Draco decompression failure?`)}function*OO(e){for(let t of e.json.meshes||[])for(let e of t.primitives)yield e}var kO=e({decode:()=>FO,name:()=>jO}),AO=`KHR_texture_transform`,jO=AO,MO=new I,NO=new L,PO=new L;async function FO(e,t){let n=new q(e);if(!n.hasExtension(AO)||!t.gltf?.loadBuffers)return;let r=e.json.materials||[];for(let t=0;t<r.length;t++)IO(t,e,n);r.some(e=>LO(e).length>0)||n.removeExtension(AO)}function IO(e,t,n){let r=t.json.materials?.[e],i=LO(r),a=new Map,o=RO(t);for(let r of i){let i=r.extensions?.[AO];if(i){let s=i.texCoord??r.texCoord??0,c=zO(s,i),l=a.get(c);if(!l){if(l={sourceTexCoord:s,texCoord:o,matrix:WO(i)},!BO(t,e,l))continue;o++,a.set(c,l)}r.texCoord=l.texCoord,n.removeObjectExtension(r,AO),r.extensions&&Object.keys(r.extensions).length===0&&delete r.extensions}}}function LO(e){if(!e||typeof e!=`object`)return[];let t=e,n=[],r=t.extensions;Number.isFinite(t.index)&&r?.[AO]&&n.push(t);for(let[e,r]of Object.entries(t))e!==`extras`&&n.push(...LO(r));return n}function RO(e){let t=-1;for(let n of e.json.meshes||[])for(let e of n.primitives)for(let n of Object.keys(e.attributes)){let e=/^TEXCOORD_(\d+)$/.exec(n);e&&(t=Math.max(t,Number(e[1])))}return t+1}function zO(e,t){let{offset:n=[0,0],rotation:r=0,scale:i=[1,1]}=t;return JSON.stringify([e,n,r,i])}function BO(e,t,n){let r=[],i=e.json.meshes||[];for(let e of i)for(let n of e.primitives){let e=n.material;Number.isFinite(e)&&t===e&&r.push(n)}if(r.length===0||r.some(t=>!VO(e,t,n.sourceTexCoord)))return!1;for(let t of r)HO(e,t,n);return!0}function VO(e,t,n){let r=t.attributes[`TEXCOORD_${n}`];if(!Number.isFinite(r))return!1;let i=e.json.accessors?.[r];if(!i||i.bufferView===void 0||i.sparse)return!1;let a=e.json.bufferViews?.[i.bufferView];return!!(a&&e.buffers[a.buffer])}function HO(e,t,n){let{sourceTexCoord:r,texCoord:i,matrix:a}=n,o=t.attributes[`TEXCOORD_${r}`];if(Number.isFinite(o)){let n=e.json.accessors?.[o];if(n&&n.bufferView!==void 0){let r=e.json.bufferViews?.[n.bufferView];if(r){let{arrayBuffer:o,byteOffset:s}=e.buffers[r.buffer],c=(s||0)+(n.byteOffset||0)+(r.byteOffset||0),{ArrayType:l,length:u}=tw(n,r),d=KC[n.componentType],f=GC[n.type],p=r.byteStride||d*f,m=new Float32Array(u);for(let e=0;e<n.count;e++){let t=new l(o,c+e*p,2);MO.set(t[0],t[1],1),MO.transformByMatrix3(a),m.set([MO[0],MO[1]],e*f)}UO(i,n,t,e,m)}}}}function UO(e,t,n,r,i){r.buffers.push({arrayBuffer:GS(i.buffer),byteOffset:0,byteLength:i.buffer.byteLength}),r.json.bufferViews=r.json.bufferViews||[];let a=r.json.bufferViews;a.push({buffer:r.buffers.length-1,byteLength:i.buffer.byteLength,byteOffset:0});let o=r.json.accessors;o&&(o.push({bufferView:a?.length-1,byteOffset:0,componentType:5126,count:t.count,type:`VEC2`}),n.attributes[`TEXCOORD_${e}`]=o.length-1)}function WO(e){let{offset:t=[0,0],rotation:n=0,scale:r=[1,1]}=e,i=new L().set(1,0,0,0,1,0,t[0],t[1],1),a=NO.set(Math.cos(n),Math.sin(n),0,-Math.sin(n),Math.cos(n),0,0,0,1),o=PO.set(r[0],0,0,0,r[1],0,0,0,1);return i.multiplyRight(a).multiplyRight(o)}var GO=e({decode:()=>JO,encode:()=>YO,name:()=>qO}),KO=`KHR_lights_punctual`,qO=KO;async function JO(e){let t=new q(e),{json:n}=t,r=t.getExtension(KO);r&&(t.json.lights=r.lights,t.removeExtension(KO));for(let e of n.nodes||[]){let n=t.getObjectExtension(e,KO);n&&(e.light=n.light),t.removeObjectExtension(e,KO)}}async function YO(e){let t=new q(e),{json:n}=t;if(n.lights){let e=t.addExtension(KO);WC(!e.lights),e.lights=n.lights,delete n.lights}if(t.json.lights){for(let e of t.json.lights){let n=e.node;t.addObjectExtension(n,KO,e)}delete t.json.lights}}var XO=e({decode:()=>$O,encode:()=>ek,name:()=>QO}),ZO=`KHR_materials_unlit`,QO=ZO;async function $O(e){let t=new q(e),{json:n}=t;for(let e of n.materials||[])e.extensions&&e.extensions.KHR_materials_unlit&&(e.unlit=!0),t.removeObjectExtension(e,ZO);t.removeExtension(ZO)}function ek(e){let t=new q(e),{json:n}=t;if(t.materials)for(let e of n.materials||[])e.unlit&&(delete e.unlit,t.addObjectExtension(e,ZO,{}),t.addExtension(ZO))}var tk=e({decode:()=>ik,encode:()=>ak,name:()=>rk}),nk=`KHR_techniques_webgl`,rk=nk;async function ik(e){let t=new q(e),{json:n}=t,r=t.getExtension(nk);if(r){let e=ok(r,t);for(let r of n.materials||[]){let n=t.getObjectExtension(r,nk);n&&(r.technique=Object.assign({},n,e[n.technique]),r.technique.values=sk(r.technique,t)),t.removeObjectExtension(r,nk)}t.removeExtension(nk)}}async function ak(e,t){}function ok(e,t){let{programs:n=[],shaders:r=[],techniques:i=[]}=e,a=new TextDecoder;return r.forEach(e=>{if(Number.isFinite(e.bufferView))e.code=a.decode(t.getTypedArrayForBufferView(e.bufferView));else throw Error(`KHR_techniques_webgl: no shader code`)}),n.forEach(e=>{e.fragmentShader=r[e.fragmentShader],e.vertexShader=r[e.vertexShader]}),i.forEach(e=>{e.program=n[e.program]}),i}function sk(e,t){let n=Object.assign({},e.values);return Object.keys(e.uniforms||{}).forEach(t=>{e.uniforms[t].value&&!(t in n)&&(n[t]=e.uniforms[t].value)}),Object.keys(n).forEach(e=>{typeof n[e]==`object`&&n[e].index!==void 0&&(n[e].texture=t.getTexture(n[e].index))}),n}var ck=[jw,bw,uD,sD,_D,pD,xD,yO,GO,XO,tk,kO,lT];async function lk(e,t={},n){let r=ck.filter(e=>dk(e.name,t));for(let i of r)await i.preprocess?.(e,t,n)}async function uk(e,t={},n){let r=ck.filter(e=>dk(e.name,t));for(let i of r)await i.decode?.(e,t,n)}function dk(e,t){let n=t?.gltf?.excludeExtensions||{};return!(e in n&&!n[e])}var fk=`KHR_binary_glTF`;function pk(e){let t=new q(e),{json:n}=t;for(let e of n.images||[]){let n=t.getObjectExtension(e,fk);n&&Object.assign(e,n),t.removeObjectExtension(e,fk)}n.buffers&&n.buffers[0]&&delete n.buffers[0].uri,t.removeExtension(fk)}var mk={accessors:`accessor`,animations:`animation`,buffers:`buffer`,bufferViews:`bufferView`,images:`image`,materials:`material`,meshes:`mesh`,nodes:`node`,samplers:`sampler`,scenes:`scene`,skins:`skin`,textures:`texture`},hk={accessor:`accessors`,animations:`animation`,buffer:`buffers`,bufferView:`bufferViews`,image:`images`,material:`materials`,mesh:`meshes`,node:`nodes`,sampler:`samplers`,scene:`scenes`,skin:`skins`,texture:`textures`},gk=class{idToIndexMap={animations:{},accessors:{},buffers:{},bufferViews:{},images:{},materials:{},meshes:{},nodes:{},samplers:{},scenes:{},skins:{},textures:{}};json;normalize(e,t){this.json=e.json;let n=e.json;switch(n.asset&&n.asset.version){case`2.0`:return;case void 0:case`1.0`:break;default:console.warn(`glTF: Unknown version ${n.asset.version}`);return}if(!t.normalize)throw Error(`glTF v1 is not supported.`);console.warn(`Converting glTF v1 to glTF v2 format. This is experimental and may fail.`),this._addAsset(n),this._convertTopLevelObjectsToArrays(n),pk(e),this._convertObjectIdsToArrayIndices(n),this._updateObjects(n),this._updateMaterial(n)}_addAsset(e){e.asset=e.asset||{},e.asset.version=`2.0`,e.asset.generator=e.asset.generator||`Normalized to glTF 2.0 by loaders.gl`}_convertTopLevelObjectsToArrays(e){for(let t in mk)this._convertTopLevelObjectToArray(e,t)}_convertTopLevelObjectToArray(e,t){let n=e[t];if(!(!n||Array.isArray(n))){e[t]=[];for(let r in n){let i=n[r];i.id=i.id||r;let a=e[t].length;e[t].push(i),this.idToIndexMap[t][r]=a}}}_convertObjectIdsToArrayIndices(e){for(let t in mk)this._convertIdsToIndices(e,t);`scene`in e&&(e.scene=this._convertIdToIndex(e.scene,`scene`));for(let t of e.textures)this._convertTextureIds(t);for(let t of e.meshes)this._convertMeshIds(t);for(let t of e.nodes)this._convertNodeIds(t);for(let t of e.scenes)this._convertSceneIds(t)}_convertTextureIds(e){e.source&&=this._convertIdToIndex(e.source,`image`)}_convertMeshIds(e){for(let t of e.primitives){let{attributes:e,indices:n,material:r}=t;for(let t in e)e[t]=this._convertIdToIndex(e[t],`accessor`);n&&(t.indices=this._convertIdToIndex(n,`accessor`)),r&&(t.material=this._convertIdToIndex(r,`material`))}}_convertNodeIds(e){e.children&&=e.children.map(e=>this._convertIdToIndex(e,`node`)),e.meshes&&=e.meshes.map(e=>this._convertIdToIndex(e,`mesh`))}_convertSceneIds(e){e.nodes&&=e.nodes.map(e=>this._convertIdToIndex(e,`node`))}_convertIdsToIndices(e,t){e[t]||(console.warn(`gltf v1: json doesn't contain attribute ${t}`),e[t]=[]);for(let n of e[t])for(let e in n){let t=n[e];n[e]=this._convertIdToIndex(t,e)}}_convertIdToIndex(e,t){let n=hk[t];if(n in this.idToIndexMap){let r=this.idToIndexMap[n][e];if(!Number.isFinite(r))throw Error(`gltf v1: failed to resolve ${t} with id ${e}`);return r}return e}_updateObjects(e){for(let e of this.json.buffers)delete e.type}_updateMaterial(e){for(let t of e.materials){t.pbrMetallicRoughness={baseColorFactor:[1,1,1,1],metallicFactor:1,roughnessFactor:1};let n=t.values?.tex||t.values?.texture2d_0||t.values?.diffuseTex,r=e.textures.findIndex(e=>e.id===n);r!==-1&&(t.pbrMetallicRoughness.baseColorTexture={index:r})}}};function _k(e,t={}){return new gk().normalize(e,t)}function vk(e,t){let n=e.basis,r=n?.format;return{...e,core:{...e.core,mimeType:t},basis:{...n,format:r&&r!==`auto`?r:NE(n?.supportedTextureFormats)}}}async function yk(e,t,n=0,r,i){return bk(e,t,n,r),_k(e,{normalize:r?.gltf?.normalize}),await lk(e,r,i),r?.gltf?.loadBuffers&&e.json.buffers&&await xk(e,r,i),r?.gltf?.loadImages&&await Sk(e,r,i),await uk(e,r,i),e}function bk(e,t,n,r){if(r.core?.baseUrl&&(e.baseUri=r.core?.baseUrl),t instanceof ArrayBuffer&&!KE(t,n,r.glb)&&(t=new TextDecoder().decode(t)),typeof t==`string`)e.json=VS(t);else if(t instanceof ArrayBuffer){let i={};n=qE(i,t,n,r.glb),WC(i.type===`glTF`,`Invalid GLB magic string ${i.type}`),e._glb=i,e.json=i.json}else WC(!1,`GLTF: must be ArrayBuffer or string`);let i=e.json.buffers||[];if(e.buffers=Array(i.length).fill(null),e._glb&&e._glb.header.hasBinChunk){let{binChunks:t}=e._glb;e.buffers[0]={arrayBuffer:t[0].arrayBuffer,byteOffset:t[0].byteOffset,byteLength:t[0].byteLength}}let a=e.json.images||[];e.images=Array(a.length).fill({})}async function xk(e,t,n){let r=e.json.buffers||[];for(let i=0;i<r.length;++i){let a=r[i];if(a.uri){let{fetch:r}=n;WC(r);let o=$E(a.uri,t,n),s=await(await n?.fetch?.(o))?.arrayBuffer?.();e.buffers[i]={arrayBuffer:s,byteOffset:0,byteLength:s.byteLength},delete a.uri}else e.buffers[i]===null&&(e.buffers[i]={arrayBuffer:new ArrayBuffer(a.byteLength),byteOffset:0,byteLength:a.byteLength})}}async function Sk(e,t,n){let r=Ck(e),i=e.json.images||[],a=[];for(let o of r)a.push(wk(e,i[o],o,t,n));return await Promise.all(a)}function Ck(e){let t=new Set,n=e.json.textures||[];for(let e of n)e.source!==void 0&&t.add(e.source);return Array.from(t).sort()}async function wk(e,t,n,r,i){let a;if(t.uri&&!t.hasOwnProperty(`bufferView`)){let e=$E(t.uri,r,i),{fetch:n}=i;a=await(await n(e)).arrayBuffer(),t.bufferView={data:a}}if(Number.isFinite(t.bufferView)){let n=rw(e.json,e.buffers,t.bufferView);a=HS(n.buffer,n.byteOffset,n.byteLength)}WC(a,`glTF image has no data`);let o=vk(r,t.mimeType),s=await NS(a,[jC,FE],o,i);s&&s[0]&&(s={compressed:!0,mipmaps:!1,width:s[0].width,height:s[0].height,data:s[0]}),e.images=e.images||[],e.images[n]=s}var Tk={dataType:null,batchType:null,name:`glTF`,id:`gltf`,module:`gltf`,version:DT,extensions:[`gltf`,`glb`],mimeTypes:[`model/gltf+json`,`model/gltf-binary`],text:!0,binary:!0,tests:[`glTF`],parse:Ek,options:{gltf:{normalize:!0,loadBuffers:!0,loadImages:!0,decompressMeshes:!0}}};async function Ek(e,t={},n){let r={...Tk.options,...t};return r.gltf={...Tk.options.gltf,...r.gltf},await yk({},e,t?.glb?.byteOffset||0,r,n)}var Dk={SCALAR:1,VEC2:2,VEC3:3,VEC4:4,MAT2:4,MAT3:9,MAT4:16},Ok={5120:1,5121:1,5122:2,5123:2,5125:4,5126:4},kk={TEXTURE_MAG_FILTER:10240,TEXTURE_MIN_FILTER:10241,TEXTURE_WRAP_S:10242,TEXTURE_WRAP_T:10243,REPEAT:10497,LINEAR:9729,NEAREST_MIPMAP_LINEAR:9986},Ak={magFilter:kk.TEXTURE_MAG_FILTER,minFilter:kk.TEXTURE_MIN_FILTER,wrapS:kk.TEXTURE_WRAP_S,wrapT:kk.TEXTURE_WRAP_T},jk={[kk.TEXTURE_MAG_FILTER]:kk.LINEAR,[kk.TEXTURE_MIN_FILTER]:kk.NEAREST_MIPMAP_LINEAR,[kk.TEXTURE_WRAP_S]:kk.REPEAT,[kk.TEXTURE_WRAP_T]:kk.REPEAT};function Mk(){return{id:`default-sampler`,parameters:jk}}function Nk(e){return Ok[e]}function Pk(e){return Dk[e]}var Fk=class{baseUri=``;jsonUnprocessed;json;buffers=[];images=[];postProcess(e,t={}){let{json:n,buffers:r=[],images:i=[]}=e,{baseUri:a=``}=e;return WC(n),this.baseUri=a,this.buffers=r,this.images=i,this.jsonUnprocessed=n,this.json=this._resolveTree(e.json,t),this.json}_resolveTree(e,t={}){let n={...e};return this.json=n,e.bufferViews&&(n.bufferViews=e.bufferViews.map((e,t)=>this._resolveBufferView(e,t))),e.images&&(n.images=e.images.map((e,t)=>this._resolveImage(e,t))),e.samplers&&(n.samplers=e.samplers.map((e,t)=>this._resolveSampler(e,t))),e.textures&&(n.textures=e.textures.map((e,t)=>this._resolveTexture(e,t))),e.accessors&&(n.accessors=e.accessors.map((e,t)=>this._resolveAccessor(e,t))),e.materials&&(n.materials=e.materials.map((e,t)=>this._resolveMaterial(e,t))),e.meshes&&(n.meshes=e.meshes.map((e,t)=>this._resolveMesh(e,t))),e.nodes&&(n.nodes=e.nodes.map((e,t)=>this._resolveNode(e,t)),n.nodes=n.nodes.map((e,t)=>this._resolveNodeChildren(e))),e.skins&&(n.skins=e.skins.map((e,t)=>this._resolveSkin(e,t))),e.scenes&&(n.scenes=e.scenes.map((e,t)=>this._resolveScene(e,t))),typeof this.json.scene==`number`&&n.scenes&&(n.scene=n.scenes[this.json.scene]),n}getScene(e){return this._get(this.json.scenes,e)}getNode(e){return this._get(this.json.nodes,e)}getSkin(e){return this._get(this.json.skins,e)}getMesh(e){return this._get(this.json.meshes,e)}getMaterial(e){return this._get(this.json.materials,e)}getAccessor(e){return this._get(this.json.accessors,e)}getCamera(e){return this._get(this.json.cameras,e)}getTexture(e){return this._get(this.json.textures,e)}getSampler(e){return this._get(this.json.samplers,e)}getImage(e){return this._get(this.json.images,e)}getBufferView(e){return this._get(this.json.bufferViews,e)}getBuffer(e){return this._get(this.json.buffers,e)}_get(e,t){if(typeof t==`object`)return t;let n=e&&e[t];return n||console.warn(`glTF file error: Could not find ${e}[${t}]`),n}_resolveScene(e,t){return{...e,id:e.id||`scene-${t}`,nodes:(e.nodes||[]).map(e=>this.getNode(e))}}_resolveNode(e,t){let n={...e,id:e?.id||`node-${t}`};return e.mesh!==void 0&&(n.mesh=this.getMesh(e.mesh)),e.camera!==void 0&&(n.camera=this.getCamera(e.camera)),e.skin!==void 0&&(n.skin=this.getSkin(e.skin)),e.meshes!==void 0&&e.meshes.length&&(n.mesh=e.meshes.reduce((e,t)=>{let n=this.getMesh(t);return e.id=n.id,e.primitives=e.primitives.concat(n.primitives),e},{primitives:[]})),n}_resolveNodeChildren(e){return e.children&&=e.children.map(e=>this.getNode(e)),e}_resolveSkin(e,t){let n=typeof e.inverseBindMatrices==`number`?this.getAccessor(e.inverseBindMatrices):void 0;return{...e,id:e.id||`skin-${t}`,inverseBindMatrices:n}}_resolveMesh(e,t){let n={...e,id:e.id||`mesh-${t}`,primitives:[]};return e.primitives&&(n.primitives=e.primitives.map((e,t)=>{let r={...e,attributes:{},indices:void 0,material:void 0},i=e.attributes;for(let e in i)r.attributes[e]=this.getAccessor(i[e]);return e.indices!==void 0&&(r.indices=this.getAccessor(e.indices)),e.material!==void 0&&(r.material=this.getMaterial(e.material)),Ik(r,n.id,t)})),n}_resolveMaterial(e,t){let n={...e,id:e.id||`material-${t}`};if(n.normalTexture&&(n.normalTexture={...n.normalTexture},n.normalTexture.texture=this.getTexture(n.normalTexture.index)),n.occlusionTexture&&(n.occlusionTexture={...n.occlusionTexture},n.occlusionTexture.texture=this.getTexture(n.occlusionTexture.index)),n.emissiveTexture&&(n.emissiveTexture={...n.emissiveTexture},n.emissiveTexture.texture=this.getTexture(n.emissiveTexture.index)),n.emissiveFactor||=n.emissiveTexture?[1,1,1]:[0,0,0],n.pbrMetallicRoughness){n.pbrMetallicRoughness={...n.pbrMetallicRoughness};let e=n.pbrMetallicRoughness;e.baseColorTexture&&(e.baseColorTexture={...e.baseColorTexture},e.baseColorTexture.texture=this.getTexture(e.baseColorTexture.index)),e.metallicRoughnessTexture&&(e.metallicRoughnessTexture={...e.metallicRoughnessTexture},e.metallicRoughnessTexture.texture=this.getTexture(e.metallicRoughnessTexture.index))}return n}_resolveAccessor(e,t){let n=Nk(e.componentType),r=Pk(e.type),i=n*r,a={...e,id:e.id||`accessor-${t}`,bytesPerComponent:n,components:r,bytesPerElement:i,value:void 0,bufferView:void 0,sparse:void 0};if(e.bufferView!==void 0&&(a.bufferView=this.getBufferView(e.bufferView)),a.bufferView){let e=a.bufferView.buffer,{ArrayType:t,byteLength:n}=tw(a,a.bufferView),r=(a.bufferView.byteOffset||0)+(a.byteOffset||0)+e.byteOffset,i=KS(e.arrayBuffer,r,n);a.bufferView.byteStride&&(i=this._getValueFromInterleavedBuffer(e,r,a.bufferView.byteStride,a.bytesPerElement,a.count)),a.value=new t(i)}else{let{ArrayType:e}=tw(a,{byteLength:a.count*a.bytesPerElement});a.value=new e(a.count*a.components)}return e.sparse&&this._applySparseAccessor(a,e.sparse),a}_applySparseAccessor(e,t){let n=Lk(t.indices.componentType),r=this._getTypedArrayFromBufferView(n,this.getBufferView(t.indices.bufferView),t.indices.byteOffset||0,t.count),i=e.value.constructor,a=this._getTypedArrayFromBufferView(i,this.getBufferView(t.values.bufferView),t.values.byteOffset||0,t.count*e.components);for(let n=0;n<t.count;n++){let t=Number(r[n]);WC(Number.isInteger(t)&&t>=0&&t<e.count,`glTF sparse accessor index is out of bounds`);for(let r=0;r<e.components;r++){let i=t*e.components+r,o=n*e.components+r;Reflect.set(e.value,i,a[o])}}}_getTypedArrayFromBufferView(e,t,n,r){let i=r*e.BYTES_PER_ELEMENT;WC(n+i<=t.byteLength,`glTF sparse accessor data exceeds its buffer view`);let a=t.buffer,o=a.byteOffset+(t.byteOffset||0)+n;return new e(KS(a.arrayBuffer,o,i))}_getValueFromInterleavedBuffer(e,t,n,r,i){let a=new Uint8Array(i*r);for(let o=0;o<i;o++){let i=t+o*n;a.set(new Uint8Array(e.arrayBuffer.slice(i,i+r)),o*r)}return a.buffer}_resolveTexture(e,t){return{...e,id:e.id||`texture-${t}`,sampler:typeof e.sampler==`number`?this.getSampler(e.sampler):Mk(),source:typeof e.source==`number`?this.getImage(e.source):void 0}}_resolveSampler(e,t){let n={id:e.id||`sampler-${t}`,...e,parameters:{}};for(let e in n){let t=this._enumSamplerParameter(e);t!==void 0&&(n.parameters[t]=n[e])}return n}_enumSamplerParameter(e){return Ak[e]}_resolveImage(e,t){let n={...e,id:e.id||`image-${t}`,image:null,bufferView:e.bufferView===void 0?void 0:this.getBufferView(e.bufferView)},r=this.images[t];return r&&(n.image=r),n}_resolveBufferView(e,t){let n=e.buffer,r=this.buffers[n].arrayBuffer,i=this.buffers[n].byteOffset||0;return e.byteOffset&&(i+=e.byteOffset),{id:`bufferView-${t}`,...e,buffer:this.buffers[n],data:new Uint8Array(r,i,e.byteLength)}}_resolveCamera(e,t){let n={...e,id:e.id||`camera-${t}`};return n.perspective,n.orthographic,n}};function Ik(e,t,n){if(e.mode!==2&&e.mode!==6)return e;let r=e.indices?.value,i=e.indices?.count??Rk(e),a=zk(r,i),o=a<=65535?Uint16Array:Uint32Array,s=e.mode===2?Hk(r,i,o):Uk(r,i,o);return e.mode=e.mode===2?1:4,e.indices={id:`${t}-primitive-${n}-portable-indices`,components:1,bytesPerComponent:o.BYTES_PER_ELEMENT,bytesPerElement:o.BYTES_PER_ELEMENT,componentType:o===Uint16Array?5123:5125,normalized:!1,count:s.length,type:`SCALAR`,min:s.length?[Bk(s)]:void 0,max:s.length?[a]:void 0,value:s},e}function Lk(e){switch(e){case 5121:return Uint8Array;case 5123:return Uint16Array;case 5125:return Uint32Array;default:throw Error(`Invalid glTF sparse index component type ${e}`)}}function Rk(e){let t=Object.values(e.attributes)[0];return WC(t,`glTF primitive must define at least one attribute`),t.count}function zk(e,t){if(!e)return Math.max(0,t-1);let n=0;for(let r=0;r<t;r++)n=Math.max(n,Number(e[r]));return n}function Bk(e){let t=1/0;for(let n of e)t=Math.min(t,n);return t}function Vk(e,t){return e?Number(e[t]):t}function Hk(e,t,n){if(t<2)return new n(0);let r=new n(t*2);for(let n=0;n<t;n++)r[n*2]=Vk(e,n),r[n*2+1]=Vk(e,(n+1)%t);return r}function Uk(e,t,n){let r=Math.max(0,t-2),i=new n(r*3);for(let t=0;t<r;t++)i[t*3]=Vk(e,0),i[t*3+1]=Vk(e,t+1),i[t*3+2]=Vk(e,t+2);return i}function Wk(e,t){return new Fk().postProcess(e,t)}async function Gk(e){let t=[];return e.scenes.forEach(e=>{e.traverse(e=>{})}),await Kk(()=>t.some(e=>!e.loaded))}async function Kk(e){for(;e();)await new Promise(e=>requestAnimationFrame(e))}var qk=`struct ScenegraphUniforms {
  sizeScale: f32,
  sizeMinPixels: f32,
  sizeMaxPixels: f32,
  sceneModelMatrix: mat4x4<f32>,
  composeModelMatrix: f32,
};

@group(0) @binding(auto)
var<uniform> scenegraph: ScenegraphUniforms;
`,Jk=`layout(std140) uniform scenegraphUniforms {
  float sizeScale;
  float sizeMinPixels;
  float sizeMaxPixels;
  mat4 sceneModelMatrix;
  float composeModelMatrix;
} scenegraph;
`,Yk={name:`scenegraph`,source:qk,vs:Jk,fs:Jk,uniformTypes:{sizeScale:`f32`,sizeMinPixels:`f32`,sizeMaxPixels:`f32`,sceneModelMatrix:`mat4x4<f32>`,composeModelMatrix:`f32`}},Xk=`#version 300 es
#define SHADER_NAME scenegraph-layer-vertex-shader
in vec3 instancePositions;
in vec3 instancePositions64Low;
in vec4 instanceColors;
in vec3 instanceModelMatrixCol0;
in vec3 instanceModelMatrixCol1;
in vec3 instanceModelMatrixCol2;
in vec3 instanceTranslation;
in vec3 positions;
#ifdef HAS_UV
in vec2 texCoords;
#endif
#ifdef LIGHTING_PBR
#ifdef HAS_NORMALS
in vec3 normals;
#endif
#endif
out vec4 vColor;
#ifndef LIGHTING_PBR
#ifdef HAS_UV
out vec2 vTEXCOORD_0;
#endif
#endif
void main(void) {
#if defined(HAS_UV) && !defined(LIGHTING_PBR)
vTEXCOORD_0 = texCoords;
geometry.uv = texCoords;
#endif
geometry.worldPosition = instancePositions;
geometry.pickingColor = picking_getPickingColorFromInstanceID();
mat3 instanceModelMatrix = mat3(instanceModelMatrixCol0, instanceModelMatrixCol1, instanceModelMatrixCol2);
vec3 normal = vec3(0.0, 0.0, 1.0);
#ifdef LIGHTING_PBR
#ifdef HAS_NORMALS
normal = instanceModelMatrix * (scenegraph.sceneModelMatrix * vec4(normals, 0.0)).xyz;
#endif
#endif
float originalSize = project_size_to_pixel(scenegraph.sizeScale);
float clampedSize = clamp(originalSize, scenegraph.sizeMinPixels, scenegraph.sizeMaxPixels);
float sizeRatio = originalSize == 0.0 ? 0.0 : clampedSize / originalSize;
vec3 pos = (instanceModelMatrix * (scenegraph.sceneModelMatrix * vec4(positions, 1.0)).xyz) * scenegraph.sizeScale * sizeRatio + instanceTranslation;
if(scenegraph.composeModelMatrix > 0.5) {
DECKGL_FILTER_SIZE(pos, geometry);
geometry.normal = project_normal(normal);
geometry.worldPosition += pos;
gl_Position = project_position_to_clipspace(pos + instancePositions, instancePositions64Low, vec3(0.0), geometry.position);
}
else {
pos = project_size(pos);
DECKGL_FILTER_SIZE(pos, geometry);
gl_Position = project_position_to_clipspace(instancePositions, instancePositions64Low, pos, geometry.position);
geometry.normal = project_normal(normal);
}
DECKGL_FILTER_GL_POSITION(gl_Position, geometry);
#ifdef LIGHTING_PBR
pbr_vPosition = geometry.position.xyz;
#ifdef HAS_NORMALS
pbr_vNormal = geometry.normal;
#endif
#ifdef HAS_UV
pbr_vUV0 = texCoords;
#else
pbr_vUV0 = vec2(0., 0.);
#endif
pbr_vUV1 = vec2(0., 0.);
geometry.uv = pbr_vUV0;
#endif
vColor = instanceColors;
DECKGL_FILTER_COLOR(vColor, geometry);
}
`,Zk=`#version 300 es
#define SHADER_NAME scenegraph-layer-fragment-shader
in vec4 vColor;
out vec4 fragColor;
#ifndef LIGHTING_PBR
#if defined(HAS_UV) && defined(HAS_BASECOLORMAP)
in vec2 vTEXCOORD_0;
uniform sampler2D pbr_baseColorSampler;
#endif
#endif
void main(void) {
#ifdef LIGHTING_PBR
fragColor = pbr_filterColor(vColor);
geometry.uv = pbr_vUV0;
#else
#if defined(HAS_UV) && defined(HAS_BASECOLORMAP)
fragColor = vColor * texture(pbr_baseColorSampler, vTEXCOORD_0);
geometry.uv = vTEXCOORD_0;
#else
fragColor = vColor;
#endif
#endif
fragColor.a *= layer.opacity;
DECKGL_FILTER_COLOR(fragColor, geometry);
}
`,Qk=`struct VertexInputs {
  @location(0) positions: vec3<f32>,
#ifdef HAS_NORMALS
  @location(1) normals: vec3<f32>,
#endif
#ifdef HAS_UV
  @location(3) texCoords: vec2<f32>,
#endif
  @location(6) instancePositions: vec3<f32>,
  @location(7) instancePositions64Low: vec3<f32>,
  @location(8) instanceColors: vec4<f32>,
  @location(10) instanceModelMatrixCol0: vec3<f32>,
  @location(11) instanceModelMatrixCol1: vec3<f32>,
  @location(12) instanceModelMatrixCol2: vec3<f32>,
  @location(13) instanceTranslation: vec3<f32>,
};

struct FragmentInputs {
  @builtin(position) position: vec4<f32>,
  @location(0) vColor: vec4<f32>,
  @location(1) vTexCoord: vec2<f32>,
  @location(2) pbrPosition: vec3<f32>,
  @location(3) pbrUV: vec2<f32>,
  @location(4) pbrNormal: vec3<f32>,
  @location(5) pickingColor: vec3<f32>,
};

@vertex
fn vertexMain(
  inputs: VertexInputs,
  @builtin(instance_index) instanceIndex: u32
) -> FragmentInputs {
  var outputs: FragmentInputs;

  geometry.worldPosition = inputs.instancePositions;
  geometry.pickingColor = picking_getPickingColorFromIndex(instanceIndex);

  var vertexPosition = inputs.positions;
  var texCoord = vec2<f32>(0.0, 0.0);
  var normal = vec3<f32>(0.0, 0.0, 1.0);

#ifdef HAS_UV
  texCoord = inputs.texCoords;
#endif
#ifdef HAS_NORMALS
  normal = inputs.normals;
#endif

  geometry.uv = texCoord;

  let instanceModelMatrix = mat3x3<f32>(
    inputs.instanceModelMatrixCol0,
    inputs.instanceModelMatrixCol1,
    inputs.instanceModelMatrixCol2
  );

  let scenePosition = (scenegraph.sceneModelMatrix * vec4<f32>(vertexPosition, 1.0)).xyz;
  let worldNormal = instanceModelMatrix * (scenegraph.sceneModelMatrix * vec4<f32>(normal, 0.0)).xyz;

  let originalSize = project_meter_size_to_pixel(scenegraph.sizeScale);
  let clampedSize = clamp(originalSize, scenegraph.sizeMinPixels, scenegraph.sizeMaxPixels);
  let sizeRatio = select(0.0, clampedSize / originalSize, originalSize > 0.0);

  let pos =
    (instanceModelMatrix * scenePosition) * scenegraph.sizeScale * sizeRatio +
    inputs.instanceTranslation;

  if (scenegraph.composeModelMatrix > 0.5) {
    geometry.normal = project_normal(worldNormal);
    geometry.worldPosition = inputs.instancePositions + pos;
    geometry.position = vec4<f32>(
      project_position_vec3_f64(inputs.instancePositions + pos, inputs.instancePositions64Low),
      1.0
    );
  } else {
    let sizeAdjustedPos = project_size_vec3(pos);
    // Scenegraph offsets are east/north/up in globe mode. Use project32's helper so it can
    // rotate the offset onto the local tangent plane before producing the common position.
    let projectResult = project_position_to_clipspace_and_commonspace(
      inputs.instancePositions,
      inputs.instancePositions64Low,
      sizeAdjustedPos
    );
    geometry.position = projectResult.commonPosition;
    geometry.normal = project_normal(worldNormal);
  }

  outputs.position = project_common_position_to_clipspace(geometry.position);
  outputs.vColor = inputs.instanceColors;
  outputs.vTexCoord = texCoord;
  outputs.pbrPosition = geometry.position.xyz;
  outputs.pbrUV = texCoord;
  outputs.pbrNormal = geometry.normal;
  outputs.pickingColor = geometry.pickingColor;
  return outputs;
}

@fragment
fn fragmentMain(inputs: FragmentInputs) -> @location(0) vec4<f32> {
  fragmentGeometry.uv = inputs.vTexCoord;

  if (picking.isActive > 0.5) {
    if (!picking_isColorValid(inputs.pickingColor)) {
      discard;
    }
    return vec4<f32>(inputs.pickingColor, 1.0);
  }

  var fragColor = inputs.vColor;

#ifdef LIGHTING_PBR
  fragmentInputs.pbr_vPosition = inputs.pbrPosition;
  // scenegraphPbrMaterial uses the indexed UV fields from the current PBR module.
  fragmentInputs.pbr_vUV0 = inputs.pbrUV;
  fragmentInputs.pbr_vUV1 = vec2<f32>(0.0);
  fragmentInputs.pbr_vNormal = inputs.pbrNormal;
  // Vertex color is part of the material base color and must be applied before lighting.
  fragColor = pbr_filterColor(fragColor);
#else
#ifdef HAS_BASECOLORMAP
  fragColor =
    fragColor *
    textureSample(pbr_baseColorSampler, pbr_baseColorSamplerSampler, inputs.vTexCoord);
#endif
#endif

  fragColor.a *= layer.opacity;

  if (picking.isHighlightActive > 0.5) {
    let highlightedObjectColor = picking_normalizeColor(picking.highlightedObjectColor);
    if (picking_isColorZero(abs(inputs.pickingColor - highlightedObjectColor))) {
      let highlightAlpha = picking.highlightColor.a;
      let blendedAlpha = highlightAlpha + fragColor.a * (1.0 - highlightAlpha);
      if (blendedAlpha > 0.0) {
        let highlightRatio = highlightAlpha / blendedAlpha;
        fragColor = vec4<f32>(
          mix(fragColor.rgb, picking.highlightColor.rgb, highlightRatio),
          blendedAlpha
        );
      }
    }
  }

  return deckgl_premultiplied_alpha(fragColor);
}
`,$k=Qr.source.replace(/fn pbr_setPositionNormalTangentUV\([\s\S]*?\n}\n/,`fn pbr_setPositionNormalTangentUV(position: vec4f, normal: vec4f, tangent: vec4f, uv: vec2f)
{
  fragmentInputs.pbr_vPosition = position.xyz;
  fragmentInputs.pbr_vNormal = normal.xyz;
  fragmentInputs.pbr_vTBN = mat3x3f(
    vec3f(1.0, 0.0, 0.0),
    vec3f(0.0, 1.0, 0.0),
    vec3f(0.0, 0.0, 1.0)
  );
  fragmentInputs.pbr_vUV0 = uv;
  fragmentInputs.pbr_vUV1 = uv;
}
`).replace(/pbrProjection\.camera/g,`project.cameraPosition`),eA={...Qr,dependencies:[jr],source:$k},tA=[255,255,255,255],nA={scenegraph:{type:`object`,value:null,async:!0},getScene:e=>e&&e.scenes?typeof e.scene==`object`?e.scene:e.scenes[e.scene||0]:e,getAnimator:e=>e&&e.animator,_animations:null,onFirstDraw:{type:`function`,value:()=>{}},sizeScale:{type:`number`,value:1,min:0},sizeMinPixels:{type:`number`,min:0,value:0},sizeMaxPixels:{type:`number`,min:0,value:2**53-1},getPosition:{type:`accessor`,value:e=>e.position},getColor:{type:`accessor`,value:tA},_lighting:`flat`,_imageBasedLightingEnvironment:void 0,getOrientation:{type:`accessor`,value:[0,0,0]},getScale:{type:`accessor`,value:[1,1,1]},getTranslation:{type:`accessor`,value:[0,0,0]},getTransformMatrix:{type:`accessor`,value:[]},loaders:[Tk]},rA=class extends Z_{getShaders(){let e={},t,n=this.context.device?.type===`webgpu`;this.props._lighting===`pbr`?(t=n?eA:Qr,e.LIGHTING_PBR=1):t=n?eA:{name:`pbrMaterial`};let r=[wd,cl,Cf,Yk,t];return super.getShaders({defines:e,vs:Xk,fs:Zk,source:Qk,modules:r})}initializeState(){let e=this.getAttributeManager(),t=this.context.device.type!==`webgpu`;e.addInstanced({instancePositions:{size:3,type:`float64`,fp64:this.use64bitPositions(),accessor:`getPosition`,transition:t},instanceColors:{type:`unorm8`,size:this.props.colorFormat.length,accessor:`getColor`,defaultValue:tA,transition:t},instanceModelMatrix:eb})}updateState(e){super.updateState(e);let{props:t,oldProps:n}=e;t.scenegraph===n.scenegraph?t._animations!==n._animations&&this._applyAnimationsProp(this.state.animator,t._animations):this._updateScenegraph()}finalizeState(e){super.finalizeState(e),this._destroyScenegraphAssets()}get isLoaded(){return!!(this.state?.scenegraph&&super.isLoaded)}_updateScenegraph(){let e=this.props,{device:t}=this.context,n=null;if(e.scenegraph instanceof ra)n={scenes:[e.scenegraph]};else if(e.scenegraph&&typeof e.scenegraph==`object`){let r=e.scenegraph,i=kS(t,r.json?Wk(r):r,this._getModelOptions());n=i,Gk(i).then(()=>this.setNeedsRedraw()).catch(e=>{this.raiseError(e,`loading glTF`)})}let r={layer:this,device:this.context.device},i=e.getScene(n,r),a=e.getAnimator(n,r);if(i instanceof sa){this._destroyScenegraphAssets(),this._applyAnimationsProp(a,e._animations);let t=[];i.traverse(e=>{e instanceof ca&&t.push(e.model)}),this.setState({scenegraph:i,animator:a,materials:n?.materials||null,models:t,firstDrawSignaled:!1}),this.getAttributeManager().invalidateAll()}else i!==null&&z.warn(`invalid scenegraph:`,i)()}_destroyScenegraphAssets(){this.state.scenegraph?.destroy(),this.state.materials?.forEach(e=>e.destroy()),this.state.scenegraph=null,this.state.animator=null,this.state.materials=null,this.state.models=[]}_applyAnimationsProp(e,t){if(!e||!t)return;let n=e.getAnimations();Object.keys(t).sort().forEach(e=>{let r=t[e];if(e===`*`)n.forEach(e=>{Object.assign(e,r)});else if(Number.isFinite(Number(e))){let t=Number(e);t>=0&&t<n.length?Object.assign(n[t],r):z.warn(`animation ${e} not found`)()}else{let t=n.find(({animation:t})=>t.name===e);t?Object.assign(t,r):z.warn(`animation ${e} not found`)()}})}_getModelOptions(){let{_imageBasedLightingEnvironment:e}=this.props,t;e&&(t=typeof e==`function`?e({device:this.context.device,gl:this.context.gl,layer:this}):e);let n=this.context.device.type===`webgpu`?{depthWriteEnabled:!0,depthCompare:`less-equal`}:void 0;return{imageBasedLightingEnvironment:t,modelOptions:{id:this.props.id,isInstanced:!0,bufferLayout:this.getAttributeManager().getBufferLayouts(),parameters:n,...this.getShaders()},useTangents:!1}}draw({context:e}){if(!this.state.scenegraph)return;this.props._animations&&this.state.animator&&(this.state.animator.setTime(e.timeline.getTime()),this.setNeedsRedraw());let{viewport:t,renderPass:n}=this.context,{sizeScale:r,sizeMinPixels:i,sizeMaxPixels:a,coordinateSystem:o}=this.props,s={camera:t.cameraPosition},c=this.getNumInstances();this.state.scenegraph.traverse((e,{worldMatrix:l})=>{if(e instanceof ca){let{model:u}=e;u.setInstanceCount(c);let d={sizeScale:r,sizeMinPixels:i,sizeMaxPixels:a,composeModelMatrix:+!!tb(t,o),sceneModelMatrix:l};u.shaderInputs.setProps({pbrProjection:s,scenegraph:d}),u.draw(n)}}),this.state.firstDrawSignaled||(this.state.firstDrawSignaled=!0,this.props.onFirstDraw?.())}};rA.defaultProps=nA,rA.layerName=`ScenegraphLayer`;var iA=`layout(std140) uniform meshUniforms {
  bool pickFeatureIds;
} mesh;
`,aA={name:`mesh`,vs:iA,fs:iA,source:`struct MeshUniforms {
  pickFeatureIds: f32,
};

@group(0) @binding(auto) var<uniform> mesh: MeshUniforms;
`,uniformTypes:{pickFeatureIds:`f32`}},oA=Qr.source.replace(/fn pbr_setPositionNormalTangentUV\([\s\S]*?\n}\n/,`fn pbr_setPositionNormalTangentUV(position: vec4f, normal: vec4f, tangent: vec4f, uv: vec2f)
{
  fragmentInputs.pbr_vPosition = position.xyz;
  fragmentInputs.pbr_vNormal = normal.xyz;
  fragmentInputs.pbr_vTBN = mat3x3f(
    vec3f(1.0, 0.0, 0.0),
    vec3f(0.0, 1.0, 0.0),
    vec3f(0.0, 0.0, 1.0)
  );
  fragmentInputs.pbr_vUV0 = uv;
  fragmentInputs.pbr_vUV1 = uv;
}
`).replace(/pbrProjection\.camera/g,`project.cameraPosition`),sA={...Qr,source:oA},cA=`#version 300 es
#define SHADER_NAME simple-mesh-layer-vs
in vec3 positions;
in vec3 normals;
in vec3 colors;
in vec2 texCoords;
in vec4 uvRegions;
in float rowIndexes;
in vec4 instanceColors;
in vec3 instanceModelMatrixCol0;
in vec3 instanceModelMatrixCol1;
in vec3 instanceModelMatrixCol2;
out vec2 vTexCoord;
out vec3 cameraPosition;
out vec3 normals_commonspace;
out vec4 position_commonspace;
out vec4 vColor;
vec2 applyUVRegion(vec2 uv) {
#ifdef HAS_UV_REGIONS
return fract(uv) * (uvRegions.zw - uvRegions.xy) + uvRegions.xy;
#else
return uv;
#endif
}
void main(void) {
vec2 uv = applyUVRegion(texCoords);
geometry.uv = uv;
if (mesh.pickFeatureIds) {
geometry.pickingColor = picking_getPickingColorFromIndex(rowIndexes);
} else {
geometry.pickingColor = picking_getPickingColorFromInstanceID();
}
mat3 instanceModelMatrix = mat3(instanceModelMatrixCol0, instanceModelMatrixCol1, instanceModelMatrixCol2);
vTexCoord = uv;
cameraPosition = project.cameraPosition;
vColor = vec4(colors * instanceColors.rgb, instanceColors.a);
vec3 pos = (instanceModelMatrix * positions) * simpleMesh.sizeScale;
vec3 projectedPosition = project_position(positions);
position_commonspace = vec4(projectedPosition, 1.0);
gl_Position = project_common_position_to_clipspace(position_commonspace);
geometry.position = position_commonspace;
normals_commonspace = project_normal(instanceModelMatrix * normals);
geometry.normal = normals_commonspace;
DECKGL_FILTER_GL_POSITION(gl_Position, geometry);
#ifdef MODULE_PBRMATERIAL
pbr_vPosition = geometry.position.xyz;
#ifdef HAS_NORMALS
pbr_vNormal = geometry.normal;
#endif
#ifdef HAS_UV
pbr_vUV0 = uv;
#else
pbr_vUV0 = vec2(0., 0.);
#endif
pbr_vUV1 = vec2(0., 0.);
geometry.uv = pbr_vUV0;
#endif
DECKGL_FILTER_COLOR(vColor, geometry);
}
`,lA=`#version 300 es
#define SHADER_NAME simple-mesh-layer-fs
precision highp float;
uniform sampler2D sampler;
in vec2 vTexCoord;
in vec3 cameraPosition;
in vec3 normals_commonspace;
in vec4 position_commonspace;
in vec4 vColor;
out vec4 fragColor;
void main(void) {
#ifdef MODULE_PBRMATERIAL
fragColor = vColor * pbr_filterColor(vec4(0));
geometry.uv = pbr_vUV0;
fragColor.a *= layer.opacity;
#else
geometry.uv = vTexCoord;
vec3 normal;
if (simpleMesh.flatShading) {
normal = normalize(cross(dFdx(position_commonspace.xyz), dFdy(position_commonspace.xyz)));
} else {
normal = normals_commonspace;
}
vec4 color = simpleMesh.hasTexture ? texture(sampler, vTexCoord) : vColor;
vec3 lightColor = lighting_getLightColor(color.rgb, cameraPosition, position_commonspace.xyz, normal);
fragColor = vec4(lightColor, color.a * layer.opacity);
#endif
DECKGL_FILTER_COLOR(fragColor, geometry);
}
`,uA=`struct VertexInputs {
  @location(0) positions: vec3<f32>,
#ifdef HAS_NORMALS
  @location(1) normals: vec3<f32>,
#endif
  @location(2) colors: vec4<f32>,
#ifdef HAS_UV
  @location(3) texCoords: vec2<f32>,
#endif
#ifdef HAS_UV_REGIONS
  @location(4) uvRegions: vec4<f32>,
#endif
#ifdef HAS_FEATURE_IDS
  @location(5) rowIndexes: u32,
#endif
  @location(6) instanceColors: vec4<f32>,
  @location(7) instanceModelMatrixCol0: vec3<f32>,
  @location(8) instanceModelMatrixCol1: vec3<f32>,
  @location(9) instanceModelMatrixCol2: vec3<f32>,
};

struct FragmentInputs {
  @builtin(position) position: vec4<f32>,
  @location(0) color: vec4<f32>,
  @location(1) texCoord: vec2<f32>,
  @location(2) pbrPosition: vec3<f32>,
  @location(3) pbrNormal: vec3<f32>,
  @location(4) pickingColor: vec3<f32>,
};

fn applyUVRegion(uv: vec2<f32>, uvRegion: vec4<f32>) -> vec2<f32> {
#ifdef HAS_UV_REGIONS
  // https://github.com/Esri/i3s-spec/blob/master/docs/1.7/geometryUVRegion.cmn.md
  return fract(uv) * (uvRegion.zw - uvRegion.xy) + uvRegion.xy;
#else
  return uv;
#endif
}

@vertex
fn vertexMain(
  inputs: VertexInputs,
  @builtin(instance_index) instanceIndex: u32
) -> FragmentInputs {
  var outputs: FragmentInputs;
  var texCoord = vec2<f32>(0.0);
  var normal = vec3<f32>(0.0, 0.0, 1.0);
  var uvRegion = vec4<f32>(0.0);

#ifdef HAS_UV
  texCoord = inputs.texCoords;
#endif
#ifdef HAS_NORMALS
  normal = inputs.normals;
#endif
#ifdef HAS_UV_REGIONS
  uvRegion = inputs.uvRegions;
#endif

  texCoord = applyUVRegion(texCoord, uvRegion);
  geometry.uv = texCoord;
#ifdef HAS_FEATURE_IDS
  geometry.pickingColor = picking_getPickingColorFromIndex(inputs.rowIndexes);
#else
  geometry.pickingColor = picking_getPickingColorFromIndex(instanceIndex);
#endif

  let instanceModelMatrix = mat3x3<f32>(
    inputs.instanceModelMatrixCol0,
    inputs.instanceModelMatrixCol1,
    inputs.instanceModelMatrixCol2
  );
  let commonPosition = vec4<f32>(project_position_vec3_f32(inputs.positions), 1.0);

  geometry.position = commonPosition;
  geometry.normal = project_normal(instanceModelMatrix * normal);

  outputs.position = project_common_position_to_clipspace(commonPosition);
  outputs.color = vec4<f32>(
    inputs.colors.rgb * inputs.instanceColors.rgb,
    inputs.instanceColors.a
  );
  outputs.texCoord = texCoord;
  outputs.pbrPosition = commonPosition.xyz;
  outputs.pbrNormal = geometry.normal;
  outputs.pickingColor = geometry.pickingColor;
  return outputs;
}

@fragment
fn fragmentMain(inputs: FragmentInputs) -> @location(0) vec4<f32> {
  fragmentGeometry.uv = inputs.texCoord;

  if (picking.isActive > 0.5) {
    if (!picking_isColorValid(inputs.pickingColor)) {
      discard;
    }
    return vec4<f32>(inputs.pickingColor, 1.0);
  }

  fragmentInputs.pbr_vPosition = inputs.pbrPosition;
  fragmentInputs.pbr_vUV0 = inputs.texCoord;
  fragmentInputs.pbr_vUV1 = vec2<f32>(0.0);
  fragmentInputs.pbr_vNormal = inputs.pbrNormal;

  var color = inputs.color * pbr_filterColor(vec4<f32>(0.0));
  color.a *= layer.opacity;

  if (picking.isHighlightActive > 0.5) {
    let highlightedObjectColor = picking_normalizeColor(picking.highlightedObjectColor);
    if (picking_isColorZero(abs(inputs.pickingColor - highlightedObjectColor))) {
      let highlightAlpha = picking.highlightColor.a;
      let blendedAlpha = highlightAlpha + color.a * (1.0 - highlightAlpha);
      if (blendedAlpha > 0.0) {
        color = vec4<f32>(
          mix(color.rgb, picking.highlightColor.rgb, highlightAlpha / blendedAlpha),
          blendedAlpha
        );
      }
    }
  }

  return deckgl_premultiplied_alpha(color);
}
`;function dA(e){let t=e.positions||e.POSITION,n=t.value.length/t.size;e.COLOR_0||e.colors||(e.colors={size:4,value:new Uint8Array(n*4).fill(255),normalized:!0})}var fA={pbrMaterial:{type:`object`,value:null},featureIds:{type:`array`,value:null,optional:!0}},pA=class extends fb{getShaders(){let e=super.getShaders();return{...e,vs:cA,fs:lA,source:uA,modules:[...e.modules,sA,aA]}}initializeState(){let{featureIds:e}=this.props;super.initializeState();let t=this.getAttributeManager();e&&t.add({rowIndexes:{type:`uint32`,size:1,noAlloc:!0,update:this.calculateFeatureIdsPickingIndexes}})}updateState(e){super.updateState(e);let{props:t,oldProps:n}=e;t.pbrMaterial!==n.pbrMaterial&&this.updatePbrMaterialUniforms(t.pbrMaterial)}draw(e){let{featureIds:t}=this.props,{model:n}=this.state;if(!n)return;let r={pickFeatureIds:!!t},i={camera:this.context.viewport.cameraPosition};n.shaderInputs.setProps({pbrProjection:i,mesh:r}),super.draw(e)}getModel(e){let{id:t}=this.props,n=this.parseMaterial(this.props.pbrMaterial,e);this.setState({parsedPBRMaterial:n});let r=this.getShaders();return dA(e.attributes),new ue(this.context.device,{...this.getShaders(),id:t,geometry:e,bufferLayout:this.getAttributeManager().getBufferLayouts(),defines:{...r.defines,...n?.defines,HAS_UV_REGIONS:+!!e.attributes.uvRegions,HAS_FEATURE_IDS:+!!this.props.featureIds},parameters:n?.parameters,isInstanced:!0})}updatePbrMaterialUniforms(e){let{model:t}=this.state;if(t){let{mesh:n}=this.props,r=this.parseMaterial(e,n);this.setState({parsedPBRMaterial:r});let{pbr_baseColorSampler:i}=r.bindings,{emptyTexture:a}=this.state,o=i||a,s={...this.context.device.type===`webgpu`?{simpleMeshTexture:o}:{sampler:o},hasTexture:!!i},{camera:c,...l}={...r.bindings,...r.uniforms};t.shaderInputs.setProps({simpleMesh:s,pbrMaterial:l})}}parseMaterial(e,t){let n=!!(e.pbrMetallicRoughness&&e.pbrMetallicRoughness.baseColorTexture);return Vb(this.context.device,{unlit:n,...e},{NORMAL:t.attributes.normals,TEXCOORD_0:t.attributes.texCoords},{pbrDebug:!1,lights:!0,useTangents:!1})}calculateFeatureIdsPickingIndexes(e){let t=this.props.featureIds;e.value=new Uint32Array(t)}finalizeState(e){super.finalizeState(e),this.state.parsedPBRMaterial?.generatedTextures.forEach(e=>e.destroy()),this.setState({parsedPBRMaterial:null})}};pA.layerName=`MeshLayer`,pA.defaultProps=fA;var mA=6378137,hA=6378137,gA=6356752.314245179;mA*mA,hA*hA,gA*gA,1/mA,1/hA,1/gA,1/(mA*mA),1/(hA*hA),1/(gA*gA);function _A(e){return e}new I;function vA(e,t=[],n=_A){return`longitude`in e?(t[0]=n(e.longitude),t[1]=n(e.latitude),t[2]=e.height):`x`in e?(t[0]=n(e.x),t[1]=n(e.y),t[2]=e.z):(t[0]=n(e[0]),t[1]=n(e[1]),t[2]=e[2]),t}function yA(e,t=[]){return vA(e,t,N._cartographicRadians?_A:ke)}function bA(e,t,n=_A){return`longitude`in t?(t.longitude=n(e[0]),t.latitude=n(e[1]),t.height=e[2]):`x`in t?(t.x=n(e[0]),t.y=n(e[1]),t.z=e[2]):(t[0]=n(e[0]),t[1]=n(e[1]),t[2]=e[2]),t}function xA(e,t){return bA(e,t,N._cartographicRadians?_A:Ae)}var SA=1e-14,CA=new I,wA={up:{south:`east`,north:`west`,west:`south`,east:`north`},down:{south:`west`,north:`east`,west:`north`,east:`south`},south:{up:`west`,down:`east`,west:`down`,east:`up`},north:{up:`east`,down:`west`,west:`up`,east:`down`},west:{up:`north`,down:`south`,north:`down`,south:`up`},east:{up:`south`,down:`north`,north:`up`,south:`down`}},TA={north:[-1,0,0],east:[0,1,0],up:[0,0,1],south:[1,0,0],west:[0,-1,0],down:[0,0,-1]},EA={east:new I,north:new I,up:new I,west:new I,south:new I,down:new I},DA=new I,OA=new I,kA=new I;function AA(e,t,n,r,i,a){let o=wA[t]&&wA[t][n];Ve(o&&(!r||r===o));let s,c,l,u=CA.copy(i);if(Pe(u.x,0,SA)&&Pe(u.y,0,SA)){let e=Math.sign(u.z);s=DA.fromArray(TA[t]),t!==`east`&&t!==`west`&&s.scale(e),c=OA.fromArray(TA[n]),n!==`east`&&n!==`west`&&c.scale(e),l=kA.fromArray(TA[r]),r!==`east`&&r!==`west`&&l.scale(e)}else{let{up:i,east:a,north:o}=EA;a.set(-u.y,u.x,0).normalize(),e.geodeticSurfaceNormal(u,i),o.copy(i).cross(a);let{down:d,west:f,south:p}=EA;d.copy(i).scale(-1),f.copy(a).scale(-1),p.copy(o).scale(-1),s=EA[t],c=EA[n],l=EA[r]}return a[0]=s.x,a[1]=s.y,a[2]=s.z,a[3]=0,a[4]=c.x,a[5]=c.y,a[6]=c.z,a[7]=0,a[8]=l.x,a[9]=l.y,a[10]=l.z,a[11]=0,a[12]=u.x,a[13]=u.y,a[14]=u.z,a[15]=1,a}var jA=new I,MA=new I,NA=new I;function PA(e,t,n=[]){let{oneOverRadii:r,oneOverRadiiSquared:i,centerToleranceSquared:a}=t;jA.from(e);let o=jA.x,s=jA.y,c=jA.z,l=r.x,u=r.y,d=r.z,f=o*o*l*l,p=s*s*u*u,m=c*c*d*d,h=f+p+m,g=Math.sqrt(1/h);if(!Number.isFinite(g))return;let _=MA;if(_.copy(e).scale(g),h<a)return _.to(n);let v=i.x,y=i.y,b=i.z,x=NA;x.set(_.x*v*2,_.y*y*2,_.z*b*2);let S=(1-g)*jA.len()/(.5*x.len()),C=0,w,T,E,D;do{S-=C,w=1/(1+S*v),T=1/(1+S*y),E=1/(1+S*b);let e=w*w,t=T*T,n=E*E,r=e*w,i=t*T,a=n*E;D=f*e+p*t+m*n-1;let o=-2*(f*r*v+p*i*y+m*a*b);C=D/o}while(Math.abs(D)>rr);return jA.scale([w,T,E]).to(n)}var FA=new I,IA=new I,LA=new I,RA=new I,zA=new I,BA=new I,J=class{constructor(e=0,t=0,n=0){this.centerToleranceSquared=nr,Ve(e>=0),Ve(t>=0),Ve(n>=0),this.radii=new I(e,t,n),this.radiiSquared=new I(e*e,t*t,n*n),this.radiiToTheFourth=new I(e*e*e*e,t*t*t*t,n*n*n*n),this.oneOverRadii=new I(e===0?0:1/e,t===0?0:1/t,n===0?0:1/n),this.oneOverRadiiSquared=new I(e===0?0:1/(e*e),t===0?0:1/(t*t),n===0?0:1/(n*n)),this.minimumRadius=Math.min(e,t,n),this.maximumRadius=Math.max(e,t,n),this.radiiSquared.z!==0&&(this.squaredXOverSquaredZ=this.radiiSquared.x/this.radiiSquared.z),Object.freeze(this)}equals(e){return this===e||!!(e&&this.radii.equals(e.radii))}toString(){return this.radii.toString()}cartographicToCartesian(e,t=[0,0,0]){let n=IA,r=LA,[,,i]=e;this.geodeticSurfaceNormalCartographic(e,n),r.copy(this.radiiSquared).scale(n);let a=Math.sqrt(n.dot(r));return r.scale(1/a),n.scale(i),r.add(n),r.to(t)}cartesianToCartographic(e,t=[0,0,0]){BA.from(e);let n=this.scaleToGeodeticSurface(BA,RA);if(!n)return;let r=this.geodeticSurfaceNormal(n,IA),i=zA;return i.copy(BA).subtract(n),xA([Math.atan2(r.y,r.x),Math.asin(r.z),Math.sign(pt(i,BA))*st(i)],t)}eastNorthUpToFixedFrame(e,t=new R){return AA(this,`east`,`north`,`up`,e,t)}localFrameToFixedFrame(e,t,n,r,i=new R){return AA(this,e,t,n,r,i)}geocentricSurfaceNormal(e,t=[0,0,0]){return FA.from(e).normalize().to(t)}geodeticSurfaceNormalCartographic(e,t=[0,0,0]){let n=yA(e),r=n[0],i=n[1],a=Math.cos(i);return FA.set(a*Math.cos(r),a*Math.sin(r),Math.sin(i)).normalize(),FA.to(t)}geodeticSurfaceNormal(e,t=[0,0,0]){return FA.from(e).scale(this.oneOverRadiiSquared).normalize().to(t)}scaleToGeodeticSurface(e,t){return PA(e,this,t)}scaleToGeocentricSurface(e,t=[0,0,0]){RA.from(e);let n=RA.x,r=RA.y,i=RA.z,a=this.oneOverRadiiSquared,o=1/Math.sqrt(n*n*a.x+r*r*a.y+i*i*a.z);return RA.multiplyScalar(o).to(t)}transformPositionToScaledSpace(e,t=[0,0,0]){return RA.from(e).scale(this.oneOverRadii).to(t)}transformPositionFromScaledSpace(e,t=[0,0,0]){return RA.from(e).scale(this.radii).to(t)}getSurfaceNormalIntersectionWithZAxis(e,t=0,n=[0,0,0]){Ve(Pe(this.radii.x,this.radii.y,ir)),Ve(this.radii.z>0),RA.from(e);let r=RA.z*(1-this.squaredXOverSquaredZ);if(!(Math.abs(r)>=this.radii.z-t))return RA.set(0,0,r).to(n)}};J.WGS84=new J(mA,hA,gA);function VA(e,t){if(!e)throw Error(t||`loader assertion failed.`)}var HA=`Queued Requests`,UA=`Active Requests`,WA=`Cancelled Requests`,GA=`Queued Requests Ever`,KA=`Active Requests Ever`,qA={id:`request-scheduler`,throttleRequests:!0,maxRequests:6,debounceTime:0},JA=class{props;stats;activeRequestCount=0;requestQueue=[];requestMap=new Map;updateTimer=null;constructor(e={}){this.props={...qA,...e},this.stats=new j({id:this.props.id}),this.stats.get(HA),this.stats.get(UA),this.stats.get(WA),this.stats.get(GA),this.stats.get(KA)}setProps(e){e.throttleRequests!==void 0&&(this.props.throttleRequests=e.throttleRequests),e.maxRequests!==void 0&&(this.props.maxRequests=e.maxRequests),e.debounceTime!==void 0&&(this.props.debounceTime=e.debounceTime)}scheduleRequest(e,t=()=>0){if(!this.props.throttleRequests)return Promise.resolve({done:()=>{}});if(this.requestMap.has(e))return this.requestMap.get(e);let n={handle:e,priority:0,getPriority:t},r=new Promise(e=>(n.resolve=e,n));return this.requestQueue.push(n),this.requestMap.set(e,r),this._issueNewRequests(),r}_issueRequest(e){let{handle:t,resolve:n}=e,r=!1,i=()=>{r||(r=!0,this.requestMap.delete(t),this.activeRequestCount--,this.updateTimer===null&&(this.updateTimer=setTimeout(()=>this._issueNewRequestsAsync(),0)))};return this.activeRequestCount++,n?n({done:i}):Promise.resolve({done:i})}_issueNewRequests(){this.updateTimer!==null&&clearTimeout(this.updateTimer),this.updateTimer=setTimeout(()=>this._issueNewRequestsAsync(),this.props.debounceTime)}_issueNewRequestsAsync(){this.updateTimer!==null&&clearTimeout(this.updateTimer),this.updateTimer=null;let e=Math.max(this.props.maxRequests-this.activeRequestCount,0);if(e!==0){this._updateAllRequests();for(let t=0;t<e;++t){let e=this.requestQueue.shift();e&&this._issueRequest(e)}}}_updateAllRequests(){let e=this.requestQueue;for(let t=0;t<e.length;++t){let n=e[t];this._updateRequest(n)||(e.splice(t,1),this.requestMap.delete(n.handle),t--)}e.sort((e,t)=>e.priority-t.priority)}_updateRequest(e){return e.priority=e.getPriority(e.handle),e.priority<0?(e.resolve(null),!1):!0}};function YA(e){let t=e?e.lastIndexOf(`/`):-1;return t>=0?e.substr(0,t):``}var XA=class{item;previous;next;constructor(e,t,n){this.item=e,this.previous=t,this.next=n}},ZA=class{head=null;tail=null;_length=0;get length(){return this._length}add(e){let t=new XA(e,this.tail,null);return this.tail?(this.tail.next=t,this.tail=t):(this.head=t,this.tail=t),++this._length,t}remove(e){e&&(e.previous&&e.next?(e.previous.next=e.next,e.next.previous=e.previous):e.previous?(e.previous.next=null,this.tail=e.previous):e.next?(e.next.previous=null,this.head=e.next):(this.head=null,this.tail=null),e.next=null,e.previous=null,--this._length)}splice(e,t){e!==t&&(this.remove(t),this._insert(e,t))}_insert(e,t){let n=e.next;e.next=t,this.tail===e?this.tail=t:n.previous=t,t.next=n,t.previous=e,++this._length}},QA=class{_list;_sentinel;_trimTiles;constructor(){this._list=new ZA,this._sentinel=this._list.add(`sentinel`),this._trimTiles=!1}reset(){this._list.splice(this._list.tail,this._sentinel)}touch(e){let t=e._cacheNode;t&&this._list.splice(this._sentinel,t)}add(e,t,n){t._cacheNode||(t._cacheNode=this._list.add(t),n&&n(e,t))}unloadTile(e,t,n){let r=t._cacheNode;r&&(this._list.remove(r),t._cacheNode=null,n&&n(e,t))}unloadTiles(e,t){let n=this._trimTiles;this._trimTiles=!1;let r=this._list,i=e.maximumMemoryUsage*1024*1024,a=this._sentinel,o=r.head;for(;o!==a&&(e.gpuMemoryUsageInBytes>i||n);){let n=o.item;o=o.next,this.unloadTile(e,n,t)}}trim(){this._trimTiles=!0}};function $A(e,t){VA(e),VA(t);let{rtcCenter:n,gltfUpAxis:r}=t,{computedTransform:i,boundingVolume:{center:a}}=e,o=new R(i);switch(n&&o.translate(n),r){case`Z`:break;case`Y`:let e=new R().rotateX(Math.PI/2);o=o.multiplyRight(e);break;case`X`:let t=new R().rotateY(-Math.PI/2);o=o.multiplyRight(t);break;default:break}t.isQuantized&&o.translate(t.quantizedVolumeOffset).scale(t.quantizedVolumeScale);let s=new I(a);t.cartesianModelMatrix=o,t.cartesianOrigin=s;let c=J.WGS84.cartesianToCartographic(s,new I);t.cartographicModelMatrix=J.WGS84.eastNorthUpToFixedFrame(s).invert().multiplyRight(o),t.cartographicOrigin=c;let l=tj(t);l&&(t.cartesianModelMatrix=new R(o).multiplyRight(l.matrix),t.cartographicModelMatrix.multiplyRight(l.matrix),l.matrix=R.IDENTITY),t.coordinateSystem||(t.modelMatrix=t.cartographicModelMatrix)}var ej=1e6**2;function tj(e){let t=e.gltf;if(!t)return null;let n=typeof t.scene==`number`?t.scene:0,r=(t.scenes?.[n])?.nodes?.[0];if(!r?.matrix)return null;let i=r.matrix;return i[12]*i[12]+i[13]*i[13]+i[14]*i[14]<=ej?null:r}var nj=new I,rj=new I,ij=new ky([new Ty,new Ty,new Ty,new Ty,new Ty,new Ty]);function aj(e,t){let{cameraDirection:n,cameraUp:r,height:i}=e,{metersPerUnit:a}=e.distanceScales,o=lj(e,e.center),s=J.WGS84.eastNorthUpToFixedFrame(o),c=e.unprojectPosition(e.cameraPosition),l=J.WGS84.cartographicToCartesian(c,new I),u=new I(s.transformAsVector(new I(n).scale(a))).normalize(),d=new I(s.transformAsVector(new I(r).scale(a))).normalize();sj(e);let f=e.constructor,{longitude:p,latitude:m,width:h,bearing:g,zoom:_}=e,v=new f({longitude:p,latitude:m,height:i,width:h,bearing:g,zoom:_,pitch:0});return{camera:{position:l,direction:u,up:d},viewport:e,topDownViewport:v,height:i,cullingVolume:ij,frameNumber:t,sseDenominator:1.15}}function oj(e,t,n){if(n===0||e.length<=n)return[e,[]];let r=[],{longitude:i,latitude:a}=t.viewport;for(let[t,n]of e.entries()){let[e,o]=n.header.mbs,s=Math.abs(i-e),c=Math.abs(a-o),l=Math.sqrt(c*c+s*s);r.push([t,l])}let o=r.sort((e,t)=>e[1]-t[1]),s=[];for(let t=0;t<n;t++)s.push(e[o[t][0]]);let c=[];for(let t=n;t<o.length;t++)c.push(e[o[t][0]]);return[s,c]}function sj(e){let t=e.getFrustumPlanes(),n=cj(t.near,e.cameraPosition),r=lj(e,n),i=lj(e,e.cameraPosition,rj),a=0;ij.planes[a++].fromPointNormal(r,nj.copy(r).subtract(i));for(let i in t){if(i===`near`)continue;let o=t[i],s=lj(e,cj(o,n,rj),rj);ij.planes[a++].fromPointNormal(s,nj.copy(r).subtract(s))}}function cj(e,t,n=new I){let r=e.normal.dot(t);return n.copy(e.normal).scale(e.distance-r).add(t),n}function lj(e,t,n=new I){let r=e.unprojectPosition(t);return J.WGS84.cartographicToCartesian(r,n)}var uj=6378137,dj=6378137,fj=6356752.314245179,pj=new I;function mj(e,t){if(e instanceof Sy){let{halfAxes:n}=e,r=_j(n);return Math.log2(fj/(r+t[2]))}else if(e instanceof py){let{radius:n}=e;return Math.log2(fj/(n+t[2]))}else if(e.width&&e.height){let{width:t,height:n}=e;return(Math.log2(uj/t)+Math.log2(dj/n))/2}return 1}function hj(e,t,n){J.WGS84.cartographicToCartesian([e.xmax,e.ymax,e.zmax],pj);let r=Math.sqrt((pj[0]-n[0])**2+(pj[1]-n[1])**2+(pj[2]-n[2])**2);return Math.log2(fj/(r+t[2]))}function gj(e,t,n){let[r,i,a,o]=e;return hj({xmin:r,xmax:a,ymin:i,ymax:o,zmin:0,zmax:0},t,n)}function _j(e){e.getColumn(0,pj);let t=e.getColumn(1),n=e.getColumn(2);return pj.add(t).add(n).len()}var vj={UNLOADED:0,LOADING:1,PROCESSING:2,READY:3,EXPIRED:4,FAILED:5},yj;(function(e){e[e.ADD=1]=`ADD`,e[e.REPLACE=2]=`REPLACE`})(yj||={});var bj;(function(e){e.EMPTY=`empty`,e.SCENEGRAPH=`scenegraph`,e.POINTCLOUD=`pointcloud`,e.MESH=`mesh`})(bj||={});var xj;(function(e){e.I3S=`I3S`,e.TILES3D=`TILES3D`})(xj||={});var Sj;(function(e){e.GEOMETRIC_ERROR=`geometricError`,e.MAX_SCREEN_THRESHOLD=`maxScreenThreshold`})(Sj||={});var Cj={NOT_COMPUTED:-1,USE_OPTIMIZATION:1,SKIP_OPTIMIZATION:0};function wj(e){return e!=null}var Tj=new I,Ej=new I,Dj=new I,Oj=new I,kj=new I,Aj=new I,jj=new I,Mj=new I;function Nj(e,t,n){if(VA(e,`3D Tile: boundingVolume must be defined`),e.box)return Fj(e.box,t,n);if(e.region)return Lj(e.region);if(e.sphere)return Ij(e.sphere,t,n);throw Error(`3D Tile: boundingVolume must contain a sphere, region, or box`)}function Pj(e,t){if(e.box)return Rj(t);if(e.region){let[t,n,r,i,a,o]=e.region;return[[Me(t),Me(n),a],[Me(r),Me(i),o]]}if(e.sphere)return zj(t);throw Error(`Unkown boundingVolume type`)}function Fj(e,t,n){let r=new I(e[0],e[1],e[2]);t.transform(r,r);let i=[];if(e.length===10){let t=e.slice(3,6),n=new tr;n.fromArray(e,6);let r=new I([1,0,0]),a=new I([0,1,0]),o=new I([0,0,1]);r.transformByQuaternion(n),r.scale(t[0]),a.transformByQuaternion(n),a.scale(t[1]),o.transformByQuaternion(n),o.scale(t[2]),i=[...r.toArray(),...a.toArray(),...o.toArray()]}else i=[...e.slice(3,6),...e.slice(6,9),...e.slice(9,12)];let a=t.transformAsVector(i.slice(0,3)),o=t.transformAsVector(i.slice(3,6)),s=t.transformAsVector(i.slice(6,9)),c=new L([a[0],a[1],a[2],o[0],o[1],o[2],s[0],s[1],s[2]]);return wj(n)?(n.center=r,n.halfAxes=c,n):new Sy(r,c)}function Ij(e,t,n){let r=new I(e[0],e[1],e[2]);t.transform(r,r);let i=t.getScale(Ej),a=Math.max(Math.max(i[0],i[1]),i[2]),o=e[3]*a;return wj(n)?(n.center=r,n.radius=o,n):new py(r,o)}function Lj(e){let[t,n,r,i,a,o]=e,s=J.WGS84.cartographicToCartesian([Me(t),Me(i),a],Dj),c=J.WGS84.cartographicToCartesian([Me(r),Me(n),o],Oj),l=new I().addVectors(s,c).multiplyByScalar(.5);return J.WGS84.cartesianToCartographic(l,kj),J.WGS84.cartographicToCartesian([Me(r),kj[1],kj[2]],Aj),J.WGS84.cartographicToCartesian([kj[0],Me(i),kj[2]],jj),J.WGS84.cartographicToCartesian([kj[0],kj[1],o],Mj),Fj([...l,...Aj.subtract(l),...jj.subtract(l),...Mj.subtract(l)],new R)}function Rj(e){let t=Bj(),{halfAxes:n}=e,r=new I(n.getColumn(0)),i=new I(n.getColumn(1)),a=new I(n.getColumn(2));for(let n=0;n<2;n++){for(let n=0;n<2;n++){for(let n=0;n<2;n++)Tj.copy(e.center),Tj.add(r),Tj.add(i),Tj.add(a),Vj(t,Tj),a.negate();i.negate()}r.negate()}return t}function zj(e){let t=Bj(),{center:n,radius:r}=e,i=J.WGS84.scaleToGeodeticSurface(n,Tj),a;a=i?J.WGS84.geodeticSurfaceNormal(i):new I(0,0,1);let o=new I(a[2],-a[1],0);o.len()>0?o.normalize():o=new I(0,1,0);let s=o.clone().cross(a);for(let e of[o,s,a]){Ej.copy(e).scale(r);for(let e=0;e<2;e++)Tj.copy(n),Tj.add(Ej),Vj(t,Tj),Ej.negate()}return t}function Bj(){return[[1/0,1/0,1/0],[-1/0,-1/0,-1/0]]}function Vj(e,t){J.WGS84.cartesianToCartographic(t,Tj),e[0][0]=Math.min(e[0][0],Tj[0]),e[0][1]=Math.min(e[0][1],Tj[1]),e[0][2]=Math.min(e[0][2],Tj[2]),e[1][0]=Math.max(e[1][0],Tj[0]),e[1][1]=Math.max(e[1][1],Tj[1]),e[1][2]=Math.max(e[1][2],Tj[2])}new I,new I,new R,new I,new I,new I;function Hj(e,t){let n=e*t;return 1-Math.exp(-(n*n))}function Uj(e,t){if(e.dynamicScreenSpaceError&&e.dynamicScreenSpaceErrorComputedDensity){let n=e.dynamicScreenSpaceErrorComputedDensity,r=e.dynamicScreenSpaceErrorFactor;return Hj(t,n)*r}return 0}function Wj(e,t,n){let r=e.tileset,i=e.parent&&e.parent.lodMetricValue||e.lodMetricValue,a=n?i:e.lodMetricValue;if(a===0)return 0;let o=Math.max(e._distanceToCamera,1e-7),{height:s,sseDenominator:c}=t,{viewDistanceScale:l}=r.options,u=a*s*(l||1)/(o*c);return u-=Uj(r,o),u}var Gj=new I,Kj=new I,qj=new I,Jj=new I,Yj=new I,Xj=new R,Zj=new R;function Qj(e,t){if(e.lodMetricValue===0||isNaN(e.lodMetricValue))return`DIG`;let n=2*$j(e,t);return n<2?`OUT`:!e.header.children||n<=e.lodMetricValue?`DRAW`:e.header.children?`DIG`:`OUT`}function $j(e,t){let{topDownViewport:n}=t,r=e.header.mbs[1],i=e.header.mbs[0],a=e.header.mbs[2],o=e.header.mbs[3],s=[...e.boundingVolume.center],c=n.unprojectPosition(n.cameraPosition);J.WGS84.cartographicToCartesian(c,Gj),Kj.copy(Gj).subtract(s).normalize(),J.WGS84.eastNorthUpToFixedFrame(s,Xj),Zj.copy(Xj).invert(),qj.copy(Gj).transform(Zj);let l=Math.sqrt(qj[0]*qj[0]+qj[1]*qj[1]),u=l*l/qj[2];Jj.copy([qj[0],qj[1],u]);let d=Jj.transform(Xj).subtract(s).normalize(),f=Kj.cross(d).normalize().scale(o).add(s),p=J.WGS84.cartesianToCartographic(f),m=n.project([i,r,a]),h=n.project(p);return Yj.copy(m).subtract(h).magnitude()}function eM(e){return{assetGltfUpAxis:e.asset&&e.asset.gltfUpAxis||`Y`}}var tM=class{_map=new Map;_array;_length;constructor(e=0){this._array=Array(e),this._length=e}get length(){return this._length}set length(e){this._length=e,e>this._array.length&&(this._array.length=e)}get values(){return this._array}get(e){return VA(e<this._array.length),this._array[e]}set(e,t){VA(e>=0),e>=this.length&&(this.length=e+1),this._map.has(this._array[e])&&this._map.delete(this._array[e]),this._array[e]=t,this._map.set(t,e)}delete(e){let t=this._map.get(e);t>=0&&(this._array.splice(t,1),this._map.delete(e),this.length--)}peek(){return this._array[this._length-1]}push(e){if(!this._map.has(e)){let t=this.length++;this._array[t]=e,this._map.set(e,t)}}pop(){let e=this._array[--this.length];return this._map.delete(e),e}reserve(e){VA(e>=0),e>this._array.length&&(this._array.length=e)}resize(e){VA(e>=0),this.length=e}trim(e){e??=this.length,this._array.length=e}reset(){this._array=[],this._map=new Map,this._length=0}find(e){return this._map.has(e)}},nM={loadSiblings:!1,skipLevelOfDetail:!1,updateTransforms:!0,onTraversalEnd:()=>{},viewportTraversersMap:{},basePath:``},rM=class{options;root=null;selectedTiles={};requestedTiles={};emptyTiles={};lastUpdate=new Date().getTime();updateDebounceTime=1e3;_traversalStack=new tM;_emptyTraversalStack=new tM;_frameNumber=null;traversalFinished(e){return!0}constructor(e){this.options={...nM,...e}}traverse(e,t,n){this.root=e,this.options={...this.options,...n},this.reset(),this.updateTile(e,t),this._frameNumber=t.frameNumber,this.executeTraversal(e,t)}reset(){this.requestedTiles={},this.selectedTiles={},this.emptyTiles={},this._traversalStack.reset(),this._emptyTraversalStack.reset()}executeTraversal(e,t){let n=this._traversalStack;for(e._selectionDepth=1,n.push(e);n.length>0;){let e=n.pop(),r=!1;this.canTraverse(e,t)&&(this.updateChildTiles(e,t),r=this.updateAndPushChildren(e,t,n,e.hasRenderContent?e._selectionDepth+1:e._selectionDepth));let i=e.parent,a=!!(!i||i._shouldRefine),o=!r;e.hasRenderContent?e.refine===yj.ADD?(this.loadTile(e,t),this.selectTile(e,t)):e.refine===yj.REPLACE&&(this.loadTile(e,t),o&&this.selectTile(e,t)):(this.emptyTiles[e.id]=e,this.loadTile(e,t),o&&this.selectTile(e,t)),this.touchTile(e,t),e._shouldRefine=r&&a}let r=new Date().getTime();(this.traversalFinished(t)||r-this.lastUpdate>this.updateDebounceTime)&&(this.lastUpdate=r,this.options.onTraversalEnd(t))}updateChildTiles(e,t){let n=e.children;for(let e of n)this.updateTile(e,t)}updateAndPushChildren(e,t,n,r){let{loadSiblings:i,skipLevelOfDetail:a}=this.options,o=e.children;o.sort(this.compareDistanceToCamera.bind(this));let s=e.refine===yj.REPLACE&&e.hasRenderContent&&!a,c=!1,l=!0;for(let e of o)if(e._selectionDepth=r,e.isVisibleAndInRequestVolume?(n.find(e)&&n.delete(e),n.push(e),c=!0):(s||i)&&(this.loadTile(e,t),this.touchTile(e,t)),s){let n;if(n=e._inRequestVolume?e.hasRenderContent?e.contentAvailable:this.executeEmptyTraversal(e,t):!1,l&&=n,!l)return!1}return c||(l=!1),l}updateTile(e,t){this.updateTileVisibility(e,t)}selectTile(e,t){this.shouldSelectTile(e)&&(e._selectedFrame=t.frameNumber,this.selectedTiles[e.id]=e)}loadTile(e,t){this.shouldLoadTile(e)&&(e._requestedFrame=t.frameNumber,e._priority=e._getPriority(),this.requestedTiles[e.id]=e)}touchTile(e,t){e.tileset._cache.touch(e),e._touchedFrame=t.frameNumber}canTraverse(e,t){return e.hasChildren?e.hasTilesetContent?!e.contentExpired:this.shouldRefine(e,t):!1}shouldLoadTile(e){return e.hasUnloadedContent||e.contentExpired}shouldSelectTile(e){return e.contentAvailable&&!this.options.skipLevelOfDetail}shouldRefine(e,t,n=!1){let r=e._screenSpaceError;return n&&(r=e.getScreenSpaceError(t,!0)),r>e.tileset.memoryAdjustedScreenSpaceError}updateTileVisibility(e,t){let n=[];if(this.options.viewportTraversersMap)for(let e in this.options.viewportTraversersMap)this.options.viewportTraversersMap[e]===t.viewport.id&&n.push(e);else n.push(t.viewport.id);e.updateVisibility(t,n)}compareDistanceToCamera(e,t){return e._distanceToCamera-t._distanceToCamera}anyChildrenVisible(e,t){let n=!1;for(let r of e.children)r.updateVisibility(t),n||=r.isVisibleAndInRequestVolume;return n}executeEmptyTraversal(e,t){let n=!0,r=this._emptyTraversalStack;for(r.push(e);r.length>0;){let e=r.pop(),i=!e.hasRenderContent&&this.canTraverse(e,t),a=!e.hasRenderContent&&e.children.length===0;if(!i&&!e.contentAvailable&&!a&&(n=!1),this.updateTile(e,t),e.isVisibleAndInRequestVolume||(this.loadTile(e,t),this.touchTile(e,t)),i){let t=e.children;for(let e of t)r.push(e)}}return e.hasEmptyContent||n}},iM=new I;function aM(e){return e!=null}var oM=class{tileset;header;id;url;parent;refine;type;contentUrl;lodMetricType=`geometricError`;lodMetricValue=0;boundingVolume=null;content=null;contentState=vj.UNLOADED;gpuMemoryUsageInBytes=0;children=[];depth=0;viewportIds=[];transform=new R;extensions=null;implicitTiling=null;userData={};computedTransform;hasEmptyContent=!1;hasTilesetContent=!1;traverser=new rM({});_cacheNode=null;_frameNumber=null;_expireDate=null;_expiredContent=null;_boundingBox=void 0;_distanceToCamera=0;_screenSpaceError=0;_visibilityPlaneMask;_visible=void 0;_contentBoundingVolume;_viewerRequestVolume;_initialTransform=new R;_priority=0;_selectedFrame=0;_requestedFrame=0;_selectionDepth=0;_touchedFrame=0;_centerZDepth=0;_shouldRefine=!1;_stackLength=0;_visitedFrame=0;_inRequestVolume=!1;_lodJudge=null;tileDrawn=!0;constructor(e,t,n,r=``){this.header=t,this.tileset=e,this.id=r||t.id,this.url=t.url,this.parent=n,this.refine=this._getRefine(t.refine),this.type=t.type,this.contentUrl=t.contentUrl,this._initializeLodMetric(t),this._initializeTransforms(t),this._initializeBoundingVolumes(t),this._initializeContent(t),this._initializeRenderingState(t),Object.seal(this)}destroy(){this.header=null}isDestroyed(){return this.header===null}get selected(){return this._selectedFrame===this.tileset._frameNumber}get isVisible(){return this._visible}get isVisibleAndInRequestVolume(){return this._visible&&this._inRequestVolume}get hasRenderContent(){return!this.hasEmptyContent&&!this.hasTilesetContent}get hasChildren(){return this.children.length>0||this.header.children&&this.header.children.length>0}get contentReady(){return this.contentState===vj.READY||this.hasEmptyContent}get contentAvailable(){return!!(this.contentReady&&this.hasRenderContent||this._expiredContent&&!this.contentFailed)}get hasUnloadedContent(){return this.hasRenderContent&&this.contentUnloaded}get contentUnloaded(){return this.contentState===vj.UNLOADED}get contentExpired(){return this.contentState===vj.EXPIRED}get contentFailed(){return this.contentState===vj.FAILED}get distanceToCamera(){return this._distanceToCamera}get screenSpaceError(){return this._screenSpaceError}get boundingBox(){return this._boundingBox||=Pj(this.header.boundingVolume,this.boundingVolume),this._boundingBox}getScreenSpaceError(e,t){switch(this.tileset.type){case xj.I3S:return $j(this,e);case xj.TILES3D:return Wj(this,e,t);default:throw Error(`Unsupported tileset type`)}}unselect(){this._selectedFrame=0}_getGpuMemoryUsageInBytes(){return this.content.gpuMemoryUsageInBytes||this.content.byteLength||0}_getPriority(){let e=this.tileset._traverser,{skipLevelOfDetail:t}=e.options,n=this.refine===yj.ADD||t;if(n&&!this.isVisible&&this._visible!==void 0||this.tileset._frameNumber-this._touchedFrame>=1||this.contentState===vj.UNLOADED)return-1;let r=this.parent,i=r&&(!n||this._screenSpaceError===0||r.hasTilesetContent)?r._screenSpaceError:this._screenSpaceError,a=e.root?e.root._screenSpaceError:0;return Math.max(a-i,0)}async loadContent(){if(this.hasEmptyContent)return!1;if(this.content)return!0;this.contentExpired&&(this._expireDate=null),this.contentState=vj.LOADING;let e=await this.tileset._requestScheduler.scheduleRequest(this.id,this._getPriority.bind(this));if(!e)return this.contentState=vj.UNLOADED,!1;try{let e=this.tileset.getTileUrl(this.contentUrl),t=this.tileset.loader,n=this.tileset.loadOptions[t.id]||{},r={...this.tileset.loadOptions,[t.id]:{...n,isTileset:this.type===`json`,...this._getLoaderSpecificOptions(t.id)}};return this.content=await sc(e,t,r),this.tileset.options.contentLoader&&await this.tileset.options.contentLoader(this),this._isTileset()&&this.tileset._initializeTileHeaders(this.content,this),this.contentState=vj.READY,this._onContentLoaded(),!0}catch(e){throw this.contentState=vj.FAILED,e}finally{e.done()}}unloadContent(){return this.content&&this.content.destroy&&this.content.destroy(),this.content=null,this.header.content&&this.header.content.destroy&&this.header.content.destroy(),this.header.content=null,this.contentState=vj.UNLOADED,this.tileDrawn=!0,!0}updateVisibility(e,t){if(this._frameNumber===e.frameNumber)return;let n=this.parent,r=n?n._visibilityPlaneMask:ky.MASK_INDETERMINATE;if(this.tileset._traverser.options.updateTransforms){let e=n?n.computedTransform:this.tileset.modelMatrix;this._updateTransform(e)}this._distanceToCamera=this.distanceToTile(e),this._screenSpaceError=this.getScreenSpaceError(e,!1),this._visibilityPlaneMask=this.visibility(e,r),this._visible=this._visibilityPlaneMask!==ky.MASK_OUTSIDE,this._inRequestVolume=this.insideViewerRequestVolume(e),this._frameNumber=e.frameNumber,this.viewportIds=t}visibility(e,t){let{cullingVolume:n}=e,{boundingVolume:r}=this;return n.computeVisibilityWithPlaneMask(r,t)}contentVisibility(){return!0}distanceToTile(e){let t=this.boundingVolume;return Math.sqrt(Math.max(t.distanceSquaredTo(e.camera.position),0))}cameraSpaceZDepth({camera:e}){let t=this.boundingVolume;return iM.subVectors(t.center,e.position),e.direction.dot(iM)}insideViewerRequestVolume(e){let t=this._viewerRequestVolume;return!t||t.distanceSquaredTo(e.camera.position)<=0}updateExpiration(){if(aM(this._expireDate)&&this.contentReady&&!this.hasEmptyContent){let e=Date.now();Date.lessThan(this._expireDate,e)&&(this.contentState=vj.EXPIRED,this._expiredContent=this.content)}}get extras(){return this.header.extras}_initializeLodMetric(e){`lodMetricType`in e?this.lodMetricType=e.lodMetricType:(this.lodMetricType=this.parent&&this.parent.lodMetricType||this.tileset.lodMetricType,console.warn(`3D Tile: Required prop lodMetricType is undefined. Using parent lodMetricType`)),`lodMetricValue`in e?this.lodMetricValue=e.lodMetricValue:(this.lodMetricValue=this.parent&&this.parent.lodMetricValue||this.tileset.lodMetricValue,console.warn(`3D Tile: Required prop lodMetricValue is undefined. Using parent lodMetricValue`))}_initializeTransforms(e){this.transform=e.transform?new R(e.transform):new R;let t=this.parent,n=this.tileset,r=t&&t.computedTransform?t.computedTransform.clone():n.modelMatrix.clone();this.computedTransform=new R(r).multiplyRight(this.transform);let i=t&&t._initialTransform?t._initialTransform.clone():new R;this._initialTransform=new R(i).multiplyRight(this.transform)}_initializeBoundingVolumes(e){this._contentBoundingVolume=null,this._viewerRequestVolume=null,this._updateBoundingVolume(e)}_initializeContent(e){this.content={_tileset:this.tileset,_tile:this},this.hasEmptyContent=!0,this.contentState=vj.UNLOADED,this.hasTilesetContent=!1,e.contentUrl&&(this.content=null,this.hasEmptyContent=!1)}_initializeRenderingState(e){this.depth=e.level||(this.parent?this.parent.depth+1:0),this._shouldRefine=!1,this._distanceToCamera=0,this._centerZDepth=0,this._screenSpaceError=0,this._visibilityPlaneMask=ky.MASK_INDETERMINATE,this._visible=void 0,this._inRequestVolume=!1,this._stackLength=0,this._selectionDepth=0,this._frameNumber=0,this._touchedFrame=0,this._visitedFrame=0,this._selectedFrame=0,this._requestedFrame=0,this._priority=0}_getRefine(e){return e||this.parent&&this.parent.refine||yj.REPLACE}_isTileset(){return this.contentUrl.indexOf(`.json`)!==-1}_onContentLoaded(){switch(this.content&&this.content.type){case`vctr`:case`geom`:this.tileset._traverser.disableSkipLevelOfDetail=!0;break;default:}this._isTileset()?this.hasTilesetContent=!0:this.gpuMemoryUsageInBytes=this._getGpuMemoryUsageInBytes()}_updateBoundingVolume(e){this.boundingVolume=Nj(e.boundingVolume,this.computedTransform,this.boundingVolume);let t=e.content;t&&(t.boundingVolume&&(this._contentBoundingVolume=Nj(t.boundingVolume,this.computedTransform,this._contentBoundingVolume)),e.viewerRequestVolume&&(this._viewerRequestVolume=Nj(e.viewerRequestVolume,this.computedTransform,this._viewerRequestVolume)))}_updateTransform(e=new R){let t=e.clone().multiplyRight(this.transform);t.equals(this.computedTransform)||(this.computedTransform=t,this._updateBoundingVolume(this.header))}_getLoaderSpecificOptions(e){switch(e){case`i3s`:return{...this.tileset.options.i3s,_tileOptions:{attributeUrls:this.header.attributeUrls,textureUrl:this.header.textureUrl,textureFormat:this.header.textureFormat,textureLoaderOptions:this.header.textureLoaderOptions,materialDefinition:this.header.materialDefinition,isDracoGeometry:this.header.isDracoGeometry,mbs:this.header.mbs},_tilesetOptions:{store:this.tileset.tileset.store,attributeStorageInfo:this.tileset.tileset.attributeStorageInfo,fields:this.tileset.tileset.fields},isTileHeader:!1};default:return eM(this.tileset.tileset)}}},sM=class extends rM{compareDistanceToCamera(e,t){return t._distanceToCamera===0&&e._distanceToCamera===0?t._centerZDepth-e._centerZDepth:t._distanceToCamera-e._distanceToCamera}updateTileVisibility(e,t){if(super.updateTileVisibility(e,t),!e.isVisibleAndInRequestVolume)return;let n=e.children.length>0;if(e.hasTilesetContent&&n){let n=e.children[0];this.updateTileVisibility(n,t),e._visible=n._visible;return}if(this.meetsScreenSpaceErrorEarly(e,t)){e._visible=!1;return}let r=e.refine===yj.REPLACE,i=e._optimChildrenWithinParent===Cj.USE_OPTIMIZATION;if(r&&i&&n&&!this.anyChildrenVisible(e,t)){e._visible=!1;return}}meetsScreenSpaceErrorEarly(e,t){let{parent:n}=e;return!n||n.hasTilesetContent||n.refine!==yj.ADD?!1:!this.shouldRefine(e,t,!0)}},cM=class{frameNumberMap=new Map;register(e,t){let n=this.frameNumberMap.get(e)||new Map,r=n.get(t)||0;n.set(t,r+1),this.frameNumberMap.set(e,n)}deregister(e,t){let n=this.frameNumberMap.get(e);if(!n)return;let r=n.get(t)||1;n.set(t,r-1)}isZero(e,t){return(this.frameNumberMap.get(e)?.get(t)||0)===0}},lM={REQUESTED:`REQUESTED`,COMPLETED:`COMPLETED`,ERROR:`ERROR`},uM=class{_statusMap;pendingTilesRegister=new cM;constructor(){this._statusMap={}}add(e,t,n,r){if(!this._statusMap[t]){let{frameNumber:i,viewport:{id:a}}=r;this._statusMap[t]={request:e,callback:n,key:t,frameState:r,status:lM.REQUESTED},this.pendingTilesRegister.register(a,i),e().then(e=>{this._statusMap[t].status=lM.COMPLETED;let{frameNumber:n,viewport:{id:i}}=this._statusMap[t].frameState;this.pendingTilesRegister.deregister(i,n),this._statusMap[t].callback(e,r)}).catch(e=>{this._statusMap[t].status=lM.ERROR;let{frameNumber:r,viewport:{id:i}}=this._statusMap[t].frameState;this.pendingTilesRegister.deregister(i,r),n(e)})}}update(e,t){if(this._statusMap[e]){let{frameNumber:n,viewport:{id:r}}=this._statusMap[e].frameState;this.pendingTilesRegister.deregister(r,n);let{frameNumber:i,viewport:{id:a}}=t;this.pendingTilesRegister.register(a,i),this._statusMap[e].frameState=t}}find(e){return this._statusMap[e]}hasPendingTiles(e,t){return!this.pendingTilesRegister.isZero(e,t)}},dM=class extends rM{_tileManager;constructor(e){super(e),this._tileManager=new uM}traversalFinished(e){return!this._tileManager.hasPendingTiles(e.viewport.id,this._frameNumber||0)}shouldRefine(e,t){return e._lodJudge=Qj(e,t),e._lodJudge===`DIG`}updateChildTiles(e,t){let n=e.header.children||[],r=e.children,i=e.tileset;for(let a of n){let n=`${a.id}-${t.viewport.id}`,o=r&&r.find(e=>e.id===n);if(o)o&&this.updateTile(o,t);else{let r=()=>this._loadTile(a.id,i);this._tileManager.find(n)?this._tileManager.update(n,t):(i.tileset.nodePages&&(r=()=>i.tileset.nodePagesTile.formTileFromNodePages(a.id)),this._tileManager.add(r,n,t=>this._onTileLoad(t,e,n),t))}}return!1}async _loadTile(e,t){let{loader:n}=t;return await sc(t.getTileUrl(`${t.url}/nodes/${e}`),n,{...t.loadOptions,i3s:{...t.loadOptions.i3s,isTileHeader:!0}})}_onTileLoad(e,t,n){let r=new oM(t.tileset,e,t,n);t.children.push(r);let i=this._tileManager.find(r.id).frameState;this.updateTile(r,i),this._frameNumber===i.frameNumber&&(this.traversalFinished(i)||new Date().getTime()-this.lastUpdate>this.updateDebounceTime)&&this.executeTraversal(r,i)}},fM={description:``,ellipsoid:J.WGS84,modelMatrix:new R,throttleRequests:!0,maxRequests:64,maximumMemoryUsage:32,memoryCacheOverflow:1,maximumTilesSelected:0,debounceTime:0,onTileLoad:()=>{},onTileUnload:()=>{},onTileError:()=>{},onTraversalComplete:e=>e,onUpdate:()=>{},contentLoader:void 0,viewDistanceScale:1,maximumScreenSpaceError:8,memoryAdjustedScreenSpaceError:!1,loadTiles:!0,updateTransforms:!0,viewportTraversersMap:null,loadOptions:{fetch:{}},attributions:[],basePath:``,i3s:{}},pM=`Tiles In Tileset(s)`,mM=`Tiles In Memory`,hM=`Tiles In View`,gM=`Tiles To Render`,_M=`Tiles Loaded`,vM=`Tiles Loading`,yM=`Tiles Unloaded`,bM=`Failed Tile Loads`,xM=`Points/Vertices`,SM=`Tile Memory Use`,CM=`Maximum Screen Space Error`,wM=class{options;loadOptions;type;tileset;loader;url;basePath;modelMatrix;ellipsoid;lodMetricType;lodMetricValue;refine;root=null;roots={};asset={};description=``;properties;extras=null;attributions={};credits={};stats;contentFormats={draco:!1,meshopt:!1,dds:!1,ktx2:!1};cartographicCenter=null;cartesianCenter=null;zoom=1;boundingVolume=null;dynamicScreenSpaceErrorComputedDensity=0;maximumMemoryUsage=32;gpuMemoryUsageInBytes=0;memoryAdjustedScreenSpaceError=0;_cacheBytes=0;_cacheOverflowBytes=0;_frameNumber=0;_queryParams={};_extensionsUsed=[];_tiles={};_pendingCount=0;selectedTiles=[];traverseCounter=0;geometricError=0;lastUpdatedVieports=null;_requestedTiles=[];_emptyTiles=[];frameStateData={};_traverser;_cache=new QA;_requestScheduler;_heldTiles=new Set;updatePromise=null;tilesetInitializationPromise;constructor(e,t){this.options={...fM,...t},this.tileset=e,this.loader=e.loader,this.type=e.type,this.url=e.url,this.basePath=e.basePath||YA(this.url),this.modelMatrix=this.options.modelMatrix,this.ellipsoid=this.options.ellipsoid,this.lodMetricType=e.lodMetricType,this.lodMetricValue=e.lodMetricValue,this.refine=e.root.refine,this.loadOptions=this.options.loadOptions||{},this._traverser=this._initializeTraverser(),this._requestScheduler=new JA({throttleRequests:this.options.throttleRequests,maxRequests:this.options.maxRequests}),this.memoryAdjustedScreenSpaceError=this.options.maximumScreenSpaceError,this._cacheBytes=this.options.maximumMemoryUsage*1024*1024,this._cacheOverflowBytes=this.options.memoryCacheOverflow*1024*1024,this.stats=new j({id:this.url}),this._initializeStats(),this.tilesetInitializationPromise=this._initializeTileSet(e)}destroy(){this._destroy()}isLoaded(){return this._pendingCount===0&&this._frameNumber!==0&&this._requestedTiles.length===0}get tiles(){return Object.values(this._tiles)}get frameNumber(){return this._frameNumber}get queryParams(){return new URLSearchParams(this._queryParams).toString()}setProps(e){this.options={...this.options,...e}}getTileUrl(e){if(e.startsWith(`data:`))return e;let t=e;return this.queryParams.length&&(t=`${e}${e.includes(`?`)?`&`:`?`}${this.queryParams}`),t}hasExtension(e){return this._extensionsUsed.indexOf(e)>-1}update(e=null){this.tilesetInitializationPromise.then(()=>{!e&&this.lastUpdatedVieports?e=this.lastUpdatedVieports:this.lastUpdatedVieports=e,e&&this.doUpdate(e)})}async selectTiles(e=null){return await this.tilesetInitializationPromise,e&&(this.lastUpdatedVieports=e),this.updatePromise||=new Promise(e=>{setTimeout(()=>{this.lastUpdatedVieports&&this.doUpdate(this.lastUpdatedVieports),e(this._frameNumber),this.updatePromise=null},this.options.debounceTime)}),this.updatePromise}adjustScreenSpaceError(){this.gpuMemoryUsageInBytes<this._cacheBytes?this.memoryAdjustedScreenSpaceError=Math.max(this.memoryAdjustedScreenSpaceError/1.02,this.options.maximumScreenSpaceError):this.gpuMemoryUsageInBytes>this._cacheBytes+this._cacheOverflowBytes&&(this.memoryAdjustedScreenSpaceError*=1.02)}doUpdate(e){if(`loadTiles`in this.options&&!this.options.loadTiles||this.traverseCounter>0)return;let t=e instanceof Array?e:[e];this._cache.reset(),this._frameNumber++,this.traverseCounter=t.length;let n=[];for(let e of t){let t=e.id;this._needTraverse(t)?n.push(t):this.traverseCounter--}for(let e of t){let t=e.id;if(this.roots[t]||(this.roots[t]=this._initializeTileHeaders(this.tileset,null)),!n.includes(t))continue;let r=aj(e,this._frameNumber);this._traverser.traverse(this.roots[t],r,this.options)}}_needTraverse(e){let t=e;return this.options.viewportTraversersMap&&(t=this.options.viewportTraversersMap[e]),t===e}_onTraversalEnd(e){let t=e.viewport.id;this.frameStateData[t]||(this.frameStateData[t]={selectedTiles:[],_requestedTiles:[],_emptyTiles:[]});let n=this.frameStateData[t],[r,i]=oj(Object.values(this._traverser.selectedTiles),e,this.options.maximumTilesSelected);n.selectedTiles=r;for(let e of i)e.unselect();n._requestedTiles=Object.values(this._traverser.requestedTiles),n._emptyTiles=Object.values(this._traverser.emptyTiles),this.traverseCounter--,!(this.traverseCounter>0)&&this._updateTiles()}_updateTiles(){let e=this.selectedTiles;this.selectedTiles=[],this._requestedTiles=[],this._emptyTiles=[];for(let e in this.frameStateData){let t=this.frameStateData[e];this.selectedTiles=this.selectedTiles.concat(t.selectedTiles),this._requestedTiles=this._requestedTiles.concat(t._requestedTiles),this._emptyTiles=this._emptyTiles.concat(t._emptyTiles)}this.selectedTiles=this.options.onTraversalComplete(this.selectedTiles);let t=new Set(this.selectedTiles.map(e=>e.id)),n=this.selectedTiles.some(e=>!e.tileDrawn),r=0;if(n){for(let e of t)this._heldTiles.add(e);for(let e of this._heldTiles){if(t.has(e))continue;let n=this._tiles[e];n&&n.contentAvailable?(n._selectedFrame=this._frameNumber,this.selectedTiles.push(n),r++):this._heldTiles.delete(e)}}else this._heldTiles=t;r>0&&setTimeout(()=>{this.selectTiles()},0);for(let e of this.selectedTiles)this._tiles[e.id]=e;this._loadTiles(),this._unloadTiles(),this._updateStats(),this._tilesChanged(e,this.selectedTiles)&&this.options.onUpdate()}_tilesChanged(e,t){if(e.length!==t.length)return!0;let n=new Set(e.map(e=>e.id)),r=new Set(t.map(e=>e.id)),i=e.filter(e=>!r.has(e.id)).length>0;return i||=t.filter(e=>!n.has(e.id)).length>0,i}_loadTiles(){this._requestedTiles.sort((e,t)=>e._priority-t._priority);for(let e of this._requestedTiles)e.contentUnloaded&&this._loadTile(e)}_unloadTiles(){this._cache.unloadTiles(this,(e,t)=>e._unloadTile(t))}_updateStats(){let e=0,t=0;for(let n of this.selectedTiles)n.contentAvailable&&n.content&&(e++,n.content.pointCount?t+=n.content.pointCount:t+=n.content.vertexCount);this.stats.get(hM).count=this.selectedTiles.length,this.stats.get(gM).count=e,this.stats.get(xM).count=t,this.stats.get(CM).count=this.memoryAdjustedScreenSpaceError}async _initializeTileSet(e){this.type===xj.I3S&&(this.calculateViewPropsI3S(),e.root=await e.root),this.root=this._initializeTileHeaders(e,null),this.type===xj.TILES3D&&(this._initializeTiles3DTileset(e),this.calculateViewPropsTiles3D()),this.type===xj.I3S&&this._initializeI3STileset()}calculateViewPropsI3S(){let e=this.tileset.fullExtent;if(e){let{xmin:t,xmax:n,ymin:r,ymax:i,zmin:a,zmax:o}=e;this.cartographicCenter=new I(t+(n-t)/2,r+(i-r)/2,a+(o-a)/2),this.cartesianCenter=new I,J.WGS84.cartographicToCartesian(this.cartographicCenter,this.cartesianCenter),this.zoom=hj(e,this.cartographicCenter,this.cartesianCenter);return}let t=this.tileset.store?.extent;if(t){let[e,n,r,i]=t;this.cartographicCenter=new I(e+(r-e)/2,n+(i-n)/2,0),this.cartesianCenter=new I,J.WGS84.cartographicToCartesian(this.cartographicCenter,this.cartesianCenter),this.zoom=gj(t,this.cartographicCenter,this.cartesianCenter);return}console.warn(`Extent is not defined in the tileset header`),this.cartographicCenter=new I,this.zoom=1}calculateViewPropsTiles3D(){let e=this.root,{center:t}=e.boundingVolume;if(!t){console.warn(`center was not pre-calculated for the root tile`),this.cartographicCenter=new I,this.zoom=1;return}t[0]!==0||t[1]!==0||t[2]!==0?(this.cartographicCenter=new I,J.WGS84.cartesianToCartographic(t,this.cartographicCenter)):this.cartographicCenter=new I(0,0,-J.WGS84.radii[0]),this.cartesianCenter=t,this.zoom=mj(e.boundingVolume,this.cartographicCenter)}_initializeStats(){this.stats.get(pM),this.stats.get(vM),this.stats.get(mM),this.stats.get(hM),this.stats.get(gM),this.stats.get(_M),this.stats.get(yM),this.stats.get(bM),this.stats.get(xM),this.stats.get(SM,`memory`),this.stats.get(CM)}_initializeTileHeaders(e,t){let n=new oM(this,e.root,t);if(t&&(t.children.push(n),n.depth=t.depth+1),this.type===xj.TILES3D){let e=[];for(e.push(n);e.length>0;){let t=e.pop();this.stats.get(pM).incrementCount();let n=t.header.children||[];for(let r of n){let n=new oM(this,r,t);if(n.contentUrl?.includes(`?session=`)){let e=new URL(n.contentUrl).searchParams.get(`session`);e&&(this._queryParams.session=e)}t.children.push(n),n.depth=t.depth+1,e.push(n)}}}return n}_initializeTraverser(){let e;switch(this.type){case xj.TILES3D:e=sM;break;case xj.I3S:e=dM;break;default:e=rM}return new e({basePath:this.basePath,onTraversalEnd:this._onTraversalEnd.bind(this)})}_destroyTileHeaders(e){this._destroySubtree(e)}async _loadTile(e){let t;try{this._onStartTileLoading(),t=await e.loadContent()}catch(t){this._onTileLoadError(e,t instanceof Error?t:Error(`load failed`))}finally{this._onEndTileLoading(),this._onTileLoad(e,t)}}_onTileLoadError(e,t){this.stats.get(bM).incrementCount();let n=t.message||t.toString(),r=e.url;console.error(`A 3D tile failed to load: ${e.url} ${n}`),this.options.onTileError(e,n,r)}_onTileLoad(e,t){if(t){if(this.type===xj.I3S){let e=this.tileset?.nodePagesTile?.nodesInNodePages||0;this.stats.get(pM).reset(),this.stats.get(pM).addCount(e)}e&&e.content&&$A(e,e.content),this.updateContentTypes(e),this._addTileToCache(e),this.options.onTileLoad(e)}}updateContentTypes(e){if(this.type===xj.I3S)switch(e.header.isDracoGeometry&&(this.contentFormats.draco=!0),e.header.textureFormat){case`dds`:this.contentFormats.dds=!0;break;case`ktx2`:this.contentFormats.ktx2=!0;break;default:}else if(this.type===xj.TILES3D){let{extensionsRemoved:t=[]}=e.content?.gltf||{};t.includes(`KHR_draco_mesh_compression`)&&(this.contentFormats.draco=!0),t.includes(`EXT_meshopt_compression`)&&(this.contentFormats.meshopt=!0),t.includes(`KHR_texture_basisu`)&&(this.contentFormats.ktx2=!0)}}_onStartTileLoading(){this._pendingCount++,this.stats.get(vM).incrementCount()}_onEndTileLoading(){this._pendingCount--,this.stats.get(vM).decrementCount()}_addTileToCache(e){this._cache.add(this,e,t=>t._updateCacheStats(e))}_updateCacheStats(e){this.stats.get(_M).incrementCount(),this.stats.get(mM).incrementCount(),this.gpuMemoryUsageInBytes+=e.gpuMemoryUsageInBytes||0,this.stats.get(SM).count=this.gpuMemoryUsageInBytes,this.options.memoryAdjustedScreenSpaceError&&this.adjustScreenSpaceError()}_unloadTile(e){this.gpuMemoryUsageInBytes-=e.gpuMemoryUsageInBytes||0,this.stats.get(mM).decrementCount(),this.stats.get(yM).incrementCount(),this.stats.get(SM).count=this.gpuMemoryUsageInBytes,this.options.onTileUnload(e),e.unloadContent()}_destroy(){let e=[];for(this.root&&e.push(this.root);e.length>0;){let t=e.pop();for(let n of t.children)e.push(n);this._destroyTile(t)}this.root=null}_destroySubtree(e){let t=e,n=[];for(n.push(t);n.length>0;){e=n.pop();for(let t of e.children)n.push(t);e!==t&&this._destroyTile(e)}t.children=[]}_destroyTile(e){this._cache.unloadTile(this,e),this._unloadTile(e),e.destroy()}_initializeTiles3DTileset(e){if(e.queryString){let t=new URLSearchParams(e.queryString),n=Object.fromEntries(t.entries());this._queryParams={...this._queryParams,...n}}if(this.asset=e.asset,!this.asset)throw Error(`Tileset must have an asset property.`);if(this.asset.version!==`0.0`&&this.asset.version!==`1.0`&&this.asset.version!==`1.1`)throw Error(`The tileset must be 3D Tiles version either 0.0 or 1.0 or 1.1.`);`tilesetVersion`in this.asset&&(this._queryParams.v=this.asset.tilesetVersion),this.credits={attributions:this.options.attributions||[]},this.description=this.options.description||``,this.properties=e.properties,this.geometricError=e.geometricError,this._extensionsUsed=e.extensionsUsed||[],this.extras=e.extras}_initializeI3STileset(){let e=this.loadOptions.i3s;e&&typeof e==`object`&&`token`in e&&(this._queryParams.token=e.token)}};async function TM(e,t,n,r){return r._parse(e,t,n,r)}function EM(e,t){if(!e)throw Error(t||`loader assertion failed.`)}function DM(e,t,n){let r=n===void 0?new Uint8Array(e).subarray(t):new Uint8Array(e).subarray(t,t+n);return new Uint8Array(r).buffer}function OM(){if(typeof process<`u`&&process.cwd!==void 0)return process.cwd();let e=window.location?.pathname;return e?.slice(0,e.lastIndexOf(`/`)+1)||``}function kM(e){let t=e?e.lastIndexOf(`/`):-1;return t>=0?e.substr(0,t):``}function AM(...e){let t=[];for(let n=0;n<e.length;n++)t[n]=e[n];let n=``,r=!1,i;for(let e=t.length-1;e>=-1&&!r;e--){let a;e>=0?a=t[e]:(i===void 0&&(i=OM()),a=i),a.length!==0&&(n=`${a}/${n}`,r=a.charCodeAt(0)===jM)}return n=NM(n,!r),r?`/${n}`:n.length>0?n:`.`}var jM=47,MM=46;function NM(e,t){let n=``,r=-1,i=0,a,o=!1;for(let s=0;s<=e.length;++s){if(s<e.length)a=e.charCodeAt(s);else if(a===jM)break;else a=jM;if(a===jM){if(!(r===s-1||i===1))if(r!==s-1&&i===2){if(n.length<2||!o||n.charCodeAt(n.length-1)!==MM||n.charCodeAt(n.length-2)!==MM){if(n.length>2){let e=n.length-1,t=e;for(;t>=0&&n.charCodeAt(t)!==jM;--t);if(t!==e){n=t===-1?``:n.slice(0,t),r=s,i=0,o=!1;continue}}else if(n.length===2||n.length===1){n=``,r=s,i=0,o=!1;continue}}t&&(n.length>0?n+=`/..`:n=`..`,o=!0)}else{let t=e.slice(r+1,s);n.length>0?n+=`/${t}`:n=t,o=!1}r=s,i=0}else a===MM&&i!==-1?++i:i=-1}return n}var PM=`4.5.4`,FM={COMPOSITE:`cmpt`,POINT_CLOUD:`pnts`,BATCHED_3D_MODEL:`b3dm`,INSTANCED_3D_MODEL:`i3dm`,GEOMETRY:`geom`,VECTOR:`vect`,GLTF:`glTF`};Object.keys(FM);function IM(e,t,n){EM(e instanceof ArrayBuffer);let r=new TextDecoder(`utf8`),i=new Uint8Array(e,t,n);return r.decode(i)}function LM(e,t=0){let n=new DataView(e);return`\
${String.fromCharCode(n.getUint8(t+0))}\
${String.fromCharCode(n.getUint8(t+1))}\
${String.fromCharCode(n.getUint8(t+2))}\
${String.fromCharCode(n.getUint8(t+3))}`}var RM={POINTS:0,LINES:1,LINE_LOOP:2,LINE_STRIP:3,TRIANGLES:4,TRIANGLE_STRIP:5,TRIANGLE_FAN:6},Y={BYTE:5120,UNSIGNED_BYTE:5121,SHORT:5122,UNSIGNED_SHORT:5123,INT:5124,UNSIGNED_INT:5125,FLOAT:5126,DOUBLE:5130},X={...RM,...Y},zM={[Y.DOUBLE]:Float64Array,[Y.FLOAT]:Float32Array,[Y.UNSIGNED_SHORT]:Uint16Array,[Y.UNSIGNED_INT]:Uint32Array,[Y.UNSIGNED_BYTE]:Uint8Array,[Y.BYTE]:Int8Array,[Y.SHORT]:Int16Array,[Y.INT]:Int32Array},BM={DOUBLE:Y.DOUBLE,FLOAT:Y.FLOAT,UNSIGNED_SHORT:Y.UNSIGNED_SHORT,UNSIGNED_INT:Y.UNSIGNED_INT,UNSIGNED_BYTE:Y.UNSIGNED_BYTE,BYTE:Y.BYTE,SHORT:Y.SHORT,INT:Y.INT},VM=`Failed to convert GL type`,HM=class e{static fromTypedArray(e){e=ArrayBuffer.isView(e)?e.constructor:e;for(let t in zM)if(zM[t]===e)return t;throw Error(VM)}static fromName(e){let t=BM[e];if(!t)throw Error(VM);return t}static getArrayType(e){switch(e){case Y.UNSIGNED_SHORT_5_6_5:case Y.UNSIGNED_SHORT_4_4_4_4:case Y.UNSIGNED_SHORT_5_5_5_1:return Uint16Array;default:let t=zM[e];if(!t)throw Error(VM);return t}}static getByteSize(t){return e.getArrayType(t).BYTES_PER_ELEMENT}static validate(t){return!!e.getArrayType(t)}static createTypedArray(t,n,r=0,i){i===void 0&&(i=(n.byteLength-r)/e.getByteSize(t));let a=ArrayBuffer.isView(n)?n.buffer:n;return new(e.getArrayType(t))(a,r,i)}};function UM(e,t){if(!e)throw Error(`math.gl assertion failed. ${t}`)}function WM(e,t=[0,0,0]){let n=e>>11&31,r=e>>5&63,i=e&31;return t[0]=n<<3,t[1]=r<<2,t[2]=i<<3,t}new at,new I,new at,new at;function GM(e,t=255){return P(e,0,t)/t*2-1}function KM(e){return e<0?-1:1}function qM(e,t,n,r){if(UM(r),e<0||e>n||t<0||t>n)throw Error(`x and y must be unsigned normalized integers between 0 and ${n}`);if(r.x=GM(e,n),r.y=GM(t,n),r.z=1-(Math.abs(r.x)+Math.abs(r.y)),r.z<0){let e=r.x;r.x=(1-Math.abs(r.y))*KM(e),r.y=(1-Math.abs(e))*KM(r.y)}return r.normalize()}function JM(e,t,n){return qM(e,t,255,n)}var YM=class{json;buffer;featuresLength=0;_cachedTypedArrays={};constructor(e,t){this.json=e,this.buffer=t}getExtension(e){return this.json.extensions&&this.json.extensions[e]}hasProperty(e){return!!this.json[e]}getGlobalProperty(e,t=X.UNSIGNED_INT,n=1){let r=this.json[e];return r&&Number.isFinite(r.byteOffset)?this._getTypedArrayFromBinary(e,t,n,1,r.byteOffset):r}getPropertyArray(e,t,n){let r=this.json[e];return r&&Number.isFinite(r.byteOffset)?(`componentType`in r&&(t=HM.fromName(r.componentType)),this._getTypedArrayFromBinary(e,t,n,this.featuresLength,r.byteOffset)):this._getTypedArrayFromArray(e,t,r)}getProperty(e,t,n,r,i){let a=this.json[e];if(!a)return a;let o=this.getPropertyArray(e,t,n);if(n===1)return o[r];for(let e=0;e<n;++e)i[e]=o[n*r+e];return i}_getTypedArrayFromBinary(e,t,n,r,i){let a=this._cachedTypedArrays,o=a[e];return o||(o=HM.createTypedArray(t,this.buffer.buffer,this.buffer.byteOffset+i,r*n),a[e]=o),o}_getTypedArrayFromArray(e,t,n){let r=this._cachedTypedArrays,i=r[e];if(!i){if(ArrayBuffer.isView(n)){let e=n.byteOffset,r=n.byteLength/HM.getByteSize(t);i=HM.createTypedArray(t,n.buffer,e,r)}else i=n instanceof ArrayBuffer?HM.createTypedArray(t,n):new(HM.getArrayType(t))(n);r[e]=i}return i}},XM={SCALAR:1,VEC2:2,VEC3:3,VEC4:4,MAT2:4,MAT3:9,MAT4:16},ZM={SCALAR:(e,t)=>e[t],VEC2:(e,t)=>[e[2*t+0],e[2*t+1]],VEC3:(e,t)=>[e[3*t+0],e[3*t+1],e[3*t+2]],VEC4:(e,t)=>[e[4*t+0],e[4*t+1],e[4*t+2],e[4*t+3]],MAT2:(e,t)=>[e[4*t+0],e[4*t+1],e[4*t+2],e[4*t+3]],MAT3:(e,t)=>[e[9*t+0],e[9*t+1],e[9*t+2],e[9*t+3],e[9*t+4],e[9*t+5],e[9*t+6],e[9*t+7],e[9*t+8]],MAT4:(e,t)=>[e[16*t+0],e[16*t+1],e[16*t+2],e[16*t+3],e[16*t+4],e[16*t+5],e[16*t+6],e[16*t+7],e[16*t+8],e[16*t+9],e[16*t+10],e[16*t+11],e[16*t+12],e[16*t+13],e[16*t+14],e[16*t+15]]},QM={SCALAR:(e,t,n)=>{t[n]=e},VEC2:(e,t,n)=>{t[2*n+0]=e[0],t[2*n+1]=e[1]},VEC3:(e,t,n)=>{t[3*n+0]=e[0],t[3*n+1]=e[1],t[3*n+2]=e[2]},VEC4:(e,t,n)=>{t[4*n+0]=e[0],t[4*n+1]=e[1],t[4*n+2]=e[2],t[4*n+3]=e[3]},MAT2:(e,t,n)=>{t[4*n+0]=e[0],t[4*n+1]=e[1],t[4*n+2]=e[2],t[4*n+3]=e[3]},MAT3:(e,t,n)=>{t[9*n+0]=e[0],t[9*n+1]=e[1],t[9*n+2]=e[2],t[9*n+3]=e[3],t[9*n+4]=e[4],t[9*n+5]=e[5],t[9*n+6]=e[6],t[9*n+7]=e[7],t[9*n+8]=e[8],t[9*n+9]=e[9]},MAT4:(e,t,n)=>{t[16*n+0]=e[0],t[16*n+1]=e[1],t[16*n+2]=e[2],t[16*n+3]=e[3],t[16*n+4]=e[4],t[16*n+5]=e[5],t[16*n+6]=e[6],t[16*n+7]=e[7],t[16*n+8]=e[8],t[16*n+9]=e[9],t[16*n+10]=e[10],t[16*n+11]=e[11],t[16*n+12]=e[12],t[16*n+13]=e[13],t[16*n+14]=e[14],t[16*n+15]=e[15]}};function $M(e,t,n,r){let{componentType:i}=e;EM(e.componentType);let a=typeof i==`string`?HM.fromName(i):i,o=XM[e.type],s=ZM[e.type],c=QM[e.type];return n+=e.byteOffset,{values:HM.createTypedArray(a,t,n,o*r),type:a,size:o,unpacker:s,packer:c}}var eN=e=>e!==void 0;function tN(e,t,n){if(!t)return null;let r=e.getExtension(`3DTILES_batch_table_hierarchy`),i=t.HIERARCHY;return i&&(console.warn(`3D Tile Parser: HIERARCHY is deprecated. Use 3DTILES_batch_table_hierarchy.`),t.extensions=t.extensions||{},t.extensions[`3DTILES_batch_table_hierarchy`]=i,r=i),r?nN(r,n):null}function nN(e,t){let n,r,i,a=e.instancesLength,o=e.classes,s=e.classIds,c=e.parentCounts,l=e.parentIds,u=a;eN(s.byteOffset)&&(s.componentType=defaultValue(s.componentType,GL.UNSIGNED_SHORT),s.type=AttributeType.SCALAR,i=getBinaryAccessor(s),s=i.createArrayBufferView(t.buffer,t.byteOffset+s.byteOffset,a));let d;if(eN(c))for(eN(c.byteOffset)&&(c.componentType=defaultValue(c.componentType,GL.UNSIGNED_SHORT),c.type=AttributeType.SCALAR,i=getBinaryAccessor(c),c=i.createArrayBufferView(t.buffer,t.byteOffset+c.byteOffset,a)),d=new Uint16Array(a),u=0,n=0;n<a;++n)d[n]=u,u+=c[n];eN(l)&&eN(l.byteOffset)&&(l.componentType=defaultValue(l.componentType,GL.UNSIGNED_SHORT),l.type=AttributeType.SCALAR,i=getBinaryAccessor(l),l=i.createArrayBufferView(t.buffer,t.byteOffset+l.byteOffset,u));let f=o.length;for(n=0;n<f;++n){let e=o[n].length,r=o[n].instances,i=getBinaryProperties(e,r,t);o[n].instances=combine(i,r)}let p=Array(f).fill(0),m=new Uint16Array(a);for(n=0;n<a;++n)r=s[n],m[n]=p[r],++p[r];let h={classes:o,classIds:s,classIndexes:m,parentCounts:c,parentIndexes:d,parentIds:l};return oN(h),h}function rN(e,t,n){if(!e)return;let r=e.parentCounts;return e.parentIds?n(e,t):r>0?iN(e,t,n):aN(e,t,n)}function iN(e,t,n){let r=e.classIds,i=e.parentCounts,a=e.parentIds,o=e.parentIndexes,s=r.length,c=scratchVisited;c.length=Math.max(c.length,s);let l=++marker,u=scratchStack;for(u.length=0,u.push(t);u.length>0;){if(t=u.pop(),c[t]===l)continue;c[t]=l;let r=n(e,t);if(eN(r))return r;let s=i[t],d=o[t];for(let e=0;e<s;++e){let n=a[d+e];n!==t&&u.push(n)}}return null}function aN(e,t,n){let r=!0;for(;r;){let i=n(e,t);if(eN(i))return i;let a=e.parentIds[t];r=a!==t,t=a}throw Error(`traverseHierarchySingleParent`)}function oN(e){let t=e.classIds.length;for(let n=0;n<t;++n)sN(e,n,stack)}function sN(e,t,n){let r=e.parentCounts,i=e.parentIds,a=e.parentIndexes,o=e.classIds.length;if(!eN(i))return;assert(t<o,`Parent index ${t} exceeds the total number of instances: ${o}`),assert(n.indexOf(t)===-1,`Circular dependency detected in the batch table hierarchy.`),n.push(t);let s=eN(r)?r[t]:1,c=eN(r)?a[t]:t;for(let r=0;r<s;++r){let a=i[c+r];a!==t&&sN(e,a,n)}n.pop(t)}function cN(e){return e!=null}var lN=(e,t)=>e,uN={HIERARCHY:!0,extensions:!0,extras:!0},dN=class{json;binary;featureCount;_extensions;_properties;_binaryProperties;_hierarchy;constructor(e,t,n,r={}){EM(n>=0),this.json=e||{},this.binary=t,this.featureCount=n,this._extensions=this.json?.extensions||{},this._properties={};for(let e in this.json)uN[e]||(this._properties[e]=this.json[e]);this._binaryProperties=this._initializeBinaryProperties(),r[`3DTILES_batch_table_hierarchy`]&&(this._hierarchy=tN(this,this.json,this.binary))}getExtension(e){return this.json&&this.json.extensions&&this.json.extensions[e]}memorySizeInBytes(){return 0}isClass(e,t){return this._checkBatchId(e),EM(typeof t==`string`,t),this._hierarchy?cN(rN(this._hierarchy,e,(e,n)=>{let r=e.classIds[n];return e.classes[r].name===t})):!1}isExactClass(e,t){return EM(typeof t==`string`,t),this.getExactClassName(e)===t}getExactClassName(e){if(this._checkBatchId(e),this._hierarchy){let t=this._hierarchy.classIds[e];return this._hierarchy.classes[t].name}}hasProperty(e,t){return this._checkBatchId(e),EM(typeof t==`string`,t),cN(this._properties[t])||this._hasPropertyInHierarchy(e,t)}getPropertyNames(e,t){this._checkBatchId(e),t=cN(t)?t:[],t.length=0;let n=Object.keys(this._properties);return t.push(...n),this._hierarchy&&this._getPropertyNamesInHierarchy(e,t),t}getProperty(e,t){if(this._checkBatchId(e),EM(typeof t==`string`,t),this._binaryProperties){let n=this._binaryProperties[t];if(cN(n))return this._getBinaryProperty(n,e)}let n=this._properties[t];if(cN(n))return lN(n[e],!0);if(this._hierarchy){let n=this._getHierarchyProperty(e,t);if(cN(n))return n}}setProperty(e,t,n){let r=this.featureCount;if(this._checkBatchId(e),EM(typeof t==`string`,t),this._binaryProperties){let r=this._binaryProperties[t];if(r){this._setBinaryProperty(r,e,n);return}}if(this._hierarchy&&this._setHierarchyProperty(this,e,t,n))return;let i=this._properties[t];cN(i)||(this._properties[t]=Array(r),i=this._properties[t]),i[e]=lN(n,!0)}_checkBatchId(e){if(!(e>=0&&e<this.featureCount))throw Error(`batchId not in range [0, featureCount - 1].`)}_getBinaryProperty(e,t){return e.unpack(e.typedArray,t)}_setBinaryProperty(e,t,n){e.pack(n,e.typedArray,t)}_initializeBinaryProperties(){let e=null;for(let t in this._properties){let n=this._properties[t],r=this._initializeBinaryProperty(t,n);r&&(e||={},e[t]=r)}return e}_initializeBinaryProperty(e,t){if(`byteOffset`in t){let n=t;EM(this.binary,`Property ${e} requires a batch table binary.`),EM(n.type,`Property ${e} requires a type.`);let r=$M(n,this.binary.buffer,this.binary.byteOffset|0,this.featureCount);return{typedArray:r.values,componentCount:r.size,unpack:r.unpacker,pack:r.packer}}return null}_hasPropertyInHierarchy(e,t){return this._hierarchy?cN(rN(this._hierarchy,e,(e,n)=>{let r=e.classIds[n],i=e.classes[r].instances;return cN(i[t])})):!1}_getPropertyNamesInHierarchy(e,t){rN(this._hierarchy,e,(e,n)=>{let r=e.classIds[n],i=e.classes[r].instances;for(let e in i)i.hasOwnProperty(e)&&t.indexOf(e)===-1&&t.push(e)})}_getHierarchyProperty(e,t){return rN(this._hierarchy,e,(e,n)=>{let r=e.classIds[n],i=e.classes[r],a=e.classIndexes[n],o=i.instances[t];return cN(o)?cN(o.typedArray)?this._getBinaryProperty(o,a):lN(o[a],!0):null})}_setHierarchyProperty(e,t,n,r){return cN(rN(this._hierarchy,t,(e,i)=>{let a=e.classIds[i],o=e.classes[a],s=e.classIndexes[i],c=o.instances[n];return cN(c)?(EM(i===t,`Inherited property "${n}" is read-only.`),cN(c.typedArray)?this._setBinaryProperty(c,s,r):c[s]=lN(r,!0),!0):!1}))}},fN=4;function pN(e,t,n=0){let r=new DataView(t);if(e.magic=r.getUint32(n,!0),n+=fN,e.version=r.getUint32(n,!0),n+=fN,e.byteLength=r.getUint32(n,!0),n+=fN,e.version!==1)throw Error(`3D Tile Version ${e.version} not supported`);return n}var mN=4,hN=`b3dm tile in legacy format.`;function gN(e,t,n){let r=new DataView(t),i;e.header=e.header||{};let a=r.getUint32(n,!0);n+=mN;let o=r.getUint32(n,!0);n+=mN;let s=r.getUint32(n,!0);n+=mN;let c=r.getUint32(n,!0);return n+=mN,s>=570425344?(n-=mN*2,i=a,s=o,c=0,a=0,o=0,console.warn(hN)):c>=570425344&&(n-=mN,i=s,s=a,c=o,a=0,o=0,console.warn(hN)),e.header.featureTableJsonByteLength=a,e.header.featureTableBinaryByteLength=o,e.header.batchTableJsonByteLength=s,e.header.batchTableBinaryByteLength=c,e.header.batchLength=i,n}function _N(e,t,n,r){return n=vN(e,t,n,r),n=yN(e,t,n,r),n}function vN(e,t,n,r){let{featureTableJsonByteLength:i,featureTableBinaryByteLength:a,batchLength:o}=e.header||{};if(e.featureTableJson={BATCH_LENGTH:o||0},i&&i>0){let r=IM(t,n,i);e.featureTableJson=JSON.parse(r)}return n+=i||0,e.featureTableBinary=new Uint8Array(t,n,a),n+=a||0,n}function yN(e,t,n,r){let{batchTableJsonByteLength:i,batchTableBinaryByteLength:a}=e.header||{};if(i&&i>0){let r=IM(t,n,i);e.batchTableJson=JSON.parse(r),n+=i,a&&a>0&&(e.batchTableBinary=new Uint8Array(t,n,a),e.batchTableBinary=new Uint8Array(e.batchTableBinary),n+=a)}return n}function bN(e,t,n){if(!t&&(!e||!e.batchIds||!n))return null;let{batchIds:r,isRGB565:i,pointCount:a=0}=e;if(r&&n){let e=new Uint8ClampedArray(a*3);for(let t=0;t<a;t++){let i=r[t],a=n.getProperty(i,`dimensions`).map(e=>e*255);e[t*3]=a[0],e[t*3+1]=a[1],e[t*3+2]=a[2]}return{type:X.UNSIGNED_BYTE,value:e,size:3,normalized:!0}}if(t&&i){let e=new Uint8ClampedArray(a*3);for(let n=0;n<a;n++){let r=WM(t[n]);e[n*3]=r[0],e[n*3+1]=r[1],e[n*3+2]=r[2]}return{type:X.UNSIGNED_BYTE,value:e,size:3,normalized:!0}}return t&&t.length===a*3?{type:X.UNSIGNED_BYTE,value:t,size:3,normalized:!0}:{type:X.UNSIGNED_BYTE,value:t||new Uint8ClampedArray,size:4,normalized:!0}}var xN=new I;function SN(e,t){if(!t)return null;if(e.isOctEncoded16P){let n=new Float32Array((e.pointsLength||0)*3);for(let r=0;r<(e.pointsLength||0);r++)JM(t[r*2],t[r*2+1],xN),xN.toArray(n,r*3);return{type:X.FLOAT,size:2,value:n}}return{type:X.FLOAT,size:2,value:t}}function CN(e,t,n){return e.isQuantized?n[`3d-tiles`]&&n[`3d-tiles`].decodeQuantizedPositions?(e.isQuantized=!1,wN(e,t)):{type:X.UNSIGNED_SHORT,value:t,size:3,normalized:!0}:t}function wN(e,t){let n=new I,r=new Float32Array(e.pointCount*3);for(let i=0;i<e.pointCount;i++)n.set(t[i*3],t[i*3+1],t[i*3+2]).scale(1/e.quantizedRange).multiply(e.quantizedVolumeScale).add(e.quantizedVolumeOffset).toArray(r,i*3);return r}async function TN(e,t,n,r,i){n=pN(e,t,n),n=gN(e,t,n),n=_N(e,t,n,r),EN(e);let{featureTable:a,batchTable:o}=DN(e);return await MN(e,a,o,r,i),ON(e,a,r),kN(e,a,o),AN(e,a),n}function EN(e){e.attributes={positions:null,colors:null,normals:null,batchIds:null},e.isQuantized=!1,e.isTranslucent=!1,e.isRGB565=!1,e.isOctEncoded16P=!1}function DN(e){let t=new YM(e.featureTableJson,e.featureTableBinary),n=t.getGlobalProperty(`POINTS_LENGTH`);if(!Number.isFinite(n))throw Error(`POINTS_LENGTH must be defined`);return t.featuresLength=n,e.featuresLength=n,e.pointsLength=n,e.pointCount=n,e.rtcCenter=t.getGlobalProperty(`RTC_CENTER`,X.FLOAT,3),{featureTable:t,batchTable:jN(e,t)}}function ON(e,t,n){if(e.attributes=e.attributes||{positions:null,colors:null,normals:null,batchIds:null},!e.attributes.positions){if(t.hasProperty(`POSITION`))e.attributes.positions=t.getPropertyArray(`POSITION`,X.FLOAT,3);else if(t.hasProperty(`POSITION_QUANTIZED`)){let r=t.getPropertyArray(`POSITION_QUANTIZED`,X.UNSIGNED_SHORT,3);if(e.isQuantized=!0,e.quantizedRange=65535,e.quantizedVolumeScale=t.getGlobalProperty(`QUANTIZED_VOLUME_SCALE`,X.FLOAT,3),!e.quantizedVolumeScale)throw Error(`QUANTIZED_VOLUME_SCALE must be defined for quantized positions.`);if(e.quantizedVolumeOffset=t.getGlobalProperty(`QUANTIZED_VOLUME_OFFSET`,X.FLOAT,3),!e.quantizedVolumeOffset)throw Error(`QUANTIZED_VOLUME_OFFSET must be defined for quantized positions.`);e.attributes.positions=CN(e,r,n)}}if(!e.attributes.positions)throw Error(`Either POSITION or POSITION_QUANTIZED must be defined.`)}function kN(e,t,n){if(e.attributes=e.attributes||{positions:null,colors:null,normals:null,batchIds:null},!e.attributes.colors){let r=null;t.hasProperty(`RGBA`)?(r=t.getPropertyArray(`RGBA`,X.UNSIGNED_BYTE,4),e.isTranslucent=!0):t.hasProperty(`RGB`)?r=t.getPropertyArray(`RGB`,X.UNSIGNED_BYTE,3):t.hasProperty(`RGB565`)&&(r=t.getPropertyArray(`RGB565`,X.UNSIGNED_SHORT,1),e.isRGB565=!0),e.attributes.colors=bN(e,r,n)}t.hasProperty(`CONSTANT_RGBA`)&&(e.constantRGBA=t.getGlobalProperty(`CONSTANT_RGBA`,X.UNSIGNED_BYTE,4))}function AN(e,t){if(e.attributes=e.attributes||{positions:null,colors:null,normals:null,batchIds:null},!e.attributes.normals){let n=null;t.hasProperty(`NORMAL`)?n=t.getPropertyArray(`NORMAL`,X.FLOAT,3):t.hasProperty(`NORMAL_OCT16P`)&&(n=t.getPropertyArray(`NORMAL_OCT16P`,X.UNSIGNED_BYTE,2),e.isOctEncoded16P=!0),e.attributes.normals=SN(e,n)}}function jN(e,t){let n=null;if(!e.batchIds&&t.hasProperty(`BATCH_ID`)&&(e.batchIds=t.getPropertyArray(`BATCH_ID`,X.UNSIGNED_SHORT,1),e.batchIds)){let r=t.getGlobalProperty(`BATCH_LENGTH`);if(!r)throw Error(`Global property: BATCH_LENGTH must be defined when BATCH_ID is defined.`);let{batchTableJson:i,batchTableBinary:a}=e;n=new dN(i,a,r)}return n}async function MN(e,t,n,r,i){let a,o,s,c=e.batchTableJson&&e.batchTableJson.extensions&&e.batchTableJson.extensions[`3DTILES_draco_point_compression`];c&&(s=c.properties);let l=t.getExtension(`3DTILES_draco_point_compression`);if(l){o=l.properties;let t=l.byteOffset,n=l.byteLength;if(!o||!Number.isFinite(t)||!n)throw Error(`Draco properties, byteOffset, and byteLength must be defined`);a=(e.featureTableBinary||[]).slice(t,t+n),e.hasPositions=Number.isFinite(o.POSITION),e.hasColors=Number.isFinite(o.RGB)||Number.isFinite(o.RGBA),e.hasNormals=Number.isFinite(o.NORMAL),e.hasBatchIds=Number.isFinite(o.BATCH_ID),e.isTranslucent=Number.isFinite(o.RGBA)}return!a||await NN(e,{buffer:a,properties:{...o,...s},featureTableProperties:o,batchTableProperties:s,dequantizeInShader:!1},r,i)}async function NN(e,t,n,r){if(!r)return;let i={...n,draco:{...n?.draco,extraAttributes:t.batchTableProperties||{}}};delete i[`3d-tiles`];let a=await TM(t.buffer,pO,i,r),o=a.attributes.POSITION&&a.attributes.POSITION.value,s=a.attributes.COLOR_0&&a.attributes.COLOR_0.value,c=a.attributes.NORMAL&&a.attributes.NORMAL.value,l=a.attributes.BATCH_ID&&a.attributes.BATCH_ID.value,u=o&&a.attributes.POSITION.value.quantization,d=c&&a.attributes.NORMAL.value.quantization;if(u){let t=a.POSITION.data.quantization,n=t.range;e.quantizedVolumeScale=new I(n,n,n),e.quantizedVolumeOffset=new I(t.minValues),e.quantizedRange=(1<<t.quantizationBits)-1,e.isQuantizedDraco=!0}d&&(e.octEncodedRange=(1<<a.NORMAL.data.quantization.quantizationBits)-1,e.isOctEncodedDraco=!0);let f={};if(t.batchTableProperties)for(let e of Object.keys(t.batchTableProperties))a.attributes[e]&&a.attributes[e].value&&(f[e.toLowerCase()]=a.attributes[e].value);e.attributes={positions:o,colors:bN(e,s,void 0),normals:c,batchIds:l,...f}}var PN={URI:0,EMBEDDED:1};function FN(e,t,n,r){e.rotateYtoZ=!0;let i=(e.byteOffset||0)+(e.byteLength||0)-n;if(i===0)throw Error(`glTF byte length must be greater than 0.`);return e.gltfUpAxis=r?.[`3d-tiles`]&&r[`3d-tiles`].assetGltfUpAxis?r[`3d-tiles`].assetGltfUpAxis:`Y`,e.gltfArrayBuffer=DM(t,n,i),e.gltfByteOffset=0,e.gltfByteLength=i,n%4==0||console.warn(`${e.type}: embedded glb is not aligned to a 4-byte boundary.`),(e.byteOffset||0)+(e.byteLength||0)}async function IN(e,t,n,r){let i=n?.[`3d-tiles`]||{};if(LN(e,t,n),i.loadGLTF){if(!r)return;if(e.gltfUrl){let{fetch:t}=r;e.gltfArrayBuffer=await(await t(e.gltfUrl,n?.core)).arrayBuffer(),e.gltfByteOffset=0}e.gltfArrayBuffer&&(e.gltf=Wk(await TM(e.gltfArrayBuffer,Tk,n,r)),e.gpuMemoryUsageInBytes=nw(e.gltf),delete e.gltfArrayBuffer,delete e.gltfByteOffset,delete e.gltfByteLength)}}function LN(e,t,n){switch(t){case PN.URI:if(e.gltfArrayBuffer){let t=new Uint8Array(e.gltfArrayBuffer,e.gltfByteOffset);e.gltfUrl=new TextDecoder().decode(t).replace(/[\s\0]+$/,``)}delete e.gltfArrayBuffer,delete e.gltfByteOffset,delete e.gltfByteLength;break;case PN.EMBEDDED:break;default:throw Error(`b3dm: Illegal glTF format field`)}}async function RN(e,t,n,r,i){n=zN(e,t,n,r,i),await IN(e,PN.EMBEDDED,r,i);let a=e?.gltf?.extensions;return a&&a.CESIUM_RTC&&(e.rtcCenter=a.CESIUM_RTC.center),n}function zN(e,t,n,r,i){return n=pN(e,t,n),n=gN(e,t,n),n=_N(e,t,n,r),n=FN(e,t,n,r),e.rtcCenter=new YM(e.featureTableJson,e.featureTableBinary).getGlobalProperty(`RTC_CENTER`,X.FLOAT,3),n}async function BN(e,t,n,r,i){return n=VN(e,t,n,r,i),await IN(e,e.gltfFormat||0,r,i),n}function VN(e,t,n,r,i){if(n=pN(e,t,n),e.version!==1)throw Error(`Instanced 3D Model version ${e.version} is not supported`);if(n=gN(e,t,n),e.gltfFormat=new DataView(t).getUint32(n,!0),n+=4,n=_N(e,t,n,r),n=FN(e,t,n,r),!e?.header?.featureTableJsonByteLength||e.header.featureTableJsonByteLength===0)throw Error(`i3dm parser: featureTableJsonByteLength is zero.`);let a=new YM(e.featureTableJson,e.featureTableBinary),o=a.getGlobalProperty(`INSTANCES_LENGTH`);if(a.featuresLength=o,!Number.isFinite(o))throw Error(`i3dm parser: INSTANCES_LENGTH must be defined`);return e.eastNorthUp=a.getGlobalProperty(`EAST_NORTH_UP`),e.rtcCenter=a.getGlobalProperty(`RTC_CENTER`,X.FLOAT,3),HN(e,a,new dN(e.batchTableJson,e.batchTableBinary,o),o),n}function HN(e,t,n,r){let i=Array(r),a=new I;new I,new I,new I;let o=new L,s=new tr,c=new I,l={},u=new R,d=[],f=[],p=[],m=[];for(let n=0;n<r;n++){let r;if(t.hasProperty(`POSITION`))r=t.getProperty(`POSITION`,X.FLOAT,3,n,a);else if(t.hasProperty(`POSITION_QUANTIZED`)){r=t.getProperty(`POSITION_QUANTIZED`,X.UNSIGNED_SHORT,3,n,a);let e=t.getGlobalProperty(`QUANTIZED_VOLUME_OFFSET`,X.FLOAT,3);if(!e)throw Error(`i3dm parser: QUANTIZED_VOLUME_OFFSET must be defined for quantized positions.`);let i=t.getGlobalProperty(`QUANTIZED_VOLUME_SCALE`,X.FLOAT,3);if(!i)throw Error(`i3dm parser: QUANTIZED_VOLUME_SCALE must be defined for quantized positions.`);for(let t=0;t<3;t++)r[t]=r[t]/65535*i[t]+e[t]}if(!r)throw Error(`i3dm: POSITION or POSITION_QUANTIZED must be defined for each instance.`);if(a.copy(r),l.translation=a,e.normalUp=t.getProperty(`NORMAL_UP`,X.FLOAT,3,n,d),e.normalRight=t.getProperty(`NORMAL_RIGHT`,X.FLOAT,3,n,f),e.normalUp){if(!e.normalRight)throw Error(`i3dm: Custom orientation requires both NORMAL_UP and NORMAL_RIGHT.`);e.hasCustomOrientation=!0}else{if(e.octNormalUp=t.getProperty(`NORMAL_UP_OCT32P`,X.UNSIGNED_SHORT,2,n,d),e.octNormalRight=t.getProperty(`NORMAL_RIGHT_OCT32P`,X.UNSIGNED_SHORT,2,n,f),e.octNormalUp)throw e.octNormalRight?Error(`i3dm: oct-encoded orientation not implemented`):Error(`i3dm: oct-encoded orientation requires NORMAL_UP_OCT32P and NORMAL_RIGHT_OCT32P`);e.eastNorthUp?(J.WGS84.eastNorthUpToFixedFrame(a,u),u.getRotationMatrix3(o)):o.identity()}s.fromMatrix3(o),l.rotation=s,c.set(1,1,1);let h=t.getProperty(`SCALE`,X.FLOAT,1,n,p);Number.isFinite(h)&&c.multiplyByScalar(h);let g=t.getProperty(`SCALE_NON_UNIFORM`,X.FLOAT,3,n,d);g&&c.scale(g),l.scale=c;let _=t.getProperty(`BATCH_ID`,X.UNSIGNED_SHORT,1,n,m);_===void 0&&(_=n);let v=new R().fromQuaternion(l.rotation);u.identity(),u.translate(l.translation),u.multiplyRight(v),u.scale(l.scale),i[n]={modelMatrix:u.clone(),batchId:_}}e.instances=i}async function UN(e,t,n,r,i,a){for(n=pN(e,t,n),e.tilesLength=new DataView(t).getUint32(n,!0),n+=4,e.tiles=[];e.tiles.length<e.tilesLength&&(e.byteLength||0)-n>12;){let o={shape:`tile3d`};e.tiles.push(o),n=await a(t,n,r,i,o)}return n}async function WN(e,t,n,r){if(e.rotateYtoZ=!0,e.gltfUpAxis=n?.[`3d-tiles`]?.assetGltfUpAxis?n[`3d-tiles`].assetGltfUpAxis:`Y`,n?.[`3d-tiles`]?.loadGLTF){if(!r)return t.byteLength;e.gltf=Wk(await TM(t,Tk,n,r)),e.gpuMemoryUsageInBytes=nw(e.gltf)}else e.gltfArrayBuffer=t;return t.byteLength}async function GN(e,t=0,n,r,i={shape:`tile3d`}){switch(i.byteOffset=t,i.type=LM(e,t),i.type){case FM.COMPOSITE:return await UN(i,e,t,n,r,GN);case FM.BATCHED_3D_MODEL:return await RN(i,e,t,n,r);case FM.GLTF:return await WN(i,e,n,r);case FM.INSTANCED_3D_MODEL:return await BN(i,e,t,n,r);case FM.POINT_CLOUD:return await TN(i,e,t,n,r);default:throw Error(`3DTileLoader: unknown type ${i.type}`)}}var KN=1952609651,qN=1;async function JN(e,t,n){if(new Uint32Array(e.slice(0,4))[0]!==KN)throw Error(`Wrong subtree file magic number`);if(new Uint32Array(e.slice(4,8))[0]!==qN)throw Error(`Wrong subtree file verson, must be 1`);let r=XN(e.slice(8,16)),i=new Uint8Array(e,24,r),a=new TextDecoder(`utf8`).decode(i),o=JSON.parse(a),s=XN(e.slice(16,24)),c=new ArrayBuffer(0);if(s&&(c=e.slice(24+r)),await YN(o,o.tileAvailability,c,n),Array.isArray(o.contentAvailability))for(let e of o.contentAvailability)await YN(o,e,c,n);else await YN(o,o.contentAvailability,c,n);return await YN(o,o.childSubtreeAvailability,c,n),o}async function YN(e,t,n,r){let i=Number.isFinite(t.bitstream)?t.bitstream:t.bufferView;if(typeof i!=`number`)return;let a=e.bufferViews[i],o=e.buffers[a.buffer];if(!r?.baseUrl)throw Error(`Url is not provided`);if(!r.fetch)throw Error(`fetch is not provided`);if(o.uri){let e=`${r?.baseUrl||``}/${o.uri}`,n=await(await r.fetch(e)).arrayBuffer();t.explicitBitstream=new Uint8Array(n,a.byteOffset,a.byteLength);return}let s=e.buffers.slice(0,a.buffer).reduce((e,t)=>e+t.byteLength,0);t.explicitBitstream=new Uint8Array(n.slice(s,s+o.byteLength),a.byteOffset,a.byteLength)}function XN(e){let t=new DataView(e);return t.getUint32(0,!0)+2**32*t.getUint32(4,!0)}var ZN={dataType:null,batchType:null,id:`3d-tiles-subtree`,name:`3D Tiles Subtree`,module:`3d-tiles`,version:PM,extensions:[`subtree`],mimeTypes:[`application/octet-stream`],tests:[`subtree`],parse:JN,options:{}},QN=null;try{QN=new WebAssembly.Instance(new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0,1,13,2,96,0,1,127,96,4,127,127,127,127,1,127,3,7,6,0,1,1,1,1,1,6,6,1,127,1,65,0,11,7,50,6,3,109,117,108,0,1,5,100,105,118,95,115,0,2,5,100,105,118,95,117,0,3,5,114,101,109,95,115,0,4,5,114,101,109,95,117,0,5,8,103,101,116,95,104,105,103,104,0,0,10,191,1,6,4,0,35,0,11,36,1,1,126,32,0,173,32,1,173,66,32,134,132,32,2,173,32,3,173,66,32,134,132,126,34,4,66,32,135,167,36,0,32,4,167,11,36,1,1,126,32,0,173,32,1,173,66,32,134,132,32,2,173,32,3,173,66,32,134,132,127,34,4,66,32,135,167,36,0,32,4,167,11,36,1,1,126,32,0,173,32,1,173,66,32,134,132,32,2,173,32,3,173,66,32,134,132,128,34,4,66,32,135,167,36,0,32,4,167,11,36,1,1,126,32,0,173,32,1,173,66,32,134,132,32,2,173,32,3,173,66,32,134,132,129,34,4,66,32,135,167,36,0,32,4,167,11,36,1,1,126,32,0,173,32,1,173,66,32,134,132,32,2,173,32,3,173,66,32,134,132,130,34,4,66,32,135,167,36,0,32,4,167,11])),{}).exports}catch{}function Z(e,t,n){this.low=e|0,this.high=t|0,this.unsigned=!!n}Z.prototype.__isLong__,Object.defineProperty(Z.prototype,"__isLong__",{value:!0});function $N(e){return(e&&e.__isLong__)===!0}function eP(e){var t=Math.clz32(e&-e);return e?31-t:t}Z.isLong=$N;var tP={},nP={};function rP(e,t){var n,r,i;return t?(e>>>=0,(i=0<=e&&e<256)&&(r=nP[e],r)?r:(n=Q(e,0,!0),i&&(nP[e]=n),n)):(e|=0,(i=-128<=e&&e<128)&&(r=tP[e],r)?r:(n=Q(e,e<0?-1:0,!1),i&&(tP[e]=n),n))}Z.fromInt=rP;function iP(e,t){if(isNaN(e))return t?hP:mP;if(t){if(e<0)return hP;if(e>=dP)return bP}else{if(e<=-fP)return xP;if(e+1>=fP)return yP}return e<0?iP(-e,t).neg():Q(e%uP|0,e/uP|0,t)}Z.fromNumber=iP;function Q(e,t,n){return new Z(e,t,n)}Z.fromBits=Q;var aP=Math.pow;function oP(e,t,n){if(e.length===0)throw Error(`empty string`);if(typeof t==`number`?(n=t,t=!1):t=!!t,e===`NaN`||e===`Infinity`||e===`+Infinity`||e===`-Infinity`)return t?hP:mP;if(n||=10,n<2||36<n)throw RangeError(`radix`);var r;if((r=e.indexOf(`-`))>0)throw Error(`interior hyphen`);if(r===0)return oP(e.substring(1),t,n).neg();for(var i=iP(aP(n,8)),a=mP,o=0;o<e.length;o+=8){var s=Math.min(8,e.length-o),c=parseInt(e.substring(o,o+s),n);if(s<8){var l=iP(aP(n,s));a=a.mul(l).add(iP(c))}else a=a.mul(i),a=a.add(iP(c))}return a.unsigned=t,a}Z.fromString=oP;function sP(e,t){return typeof e==`number`?iP(e,t):typeof e==`string`?oP(e,t):Q(e.low,e.high,typeof t==`boolean`?t:e.unsigned)}Z.fromValue=sP;var cP=65536,lP=1<<24,uP=cP*cP,dP=uP*uP,fP=dP/2,pP=rP(lP),mP=rP(0);Z.ZERO=mP;var hP=rP(0,!0);Z.UZERO=hP;var gP=rP(1);Z.ONE=gP;var _P=rP(1,!0);Z.UONE=_P;var vP=rP(-1);Z.NEG_ONE=vP;var yP=Q(-1,2147483647,!1);Z.MAX_VALUE=yP;var bP=Q(-1,-1,!0);Z.MAX_UNSIGNED_VALUE=bP;var xP=Q(0,-2147483648,!1);Z.MIN_VALUE=xP;var $=Z.prototype;$.toInt=function(){return this.unsigned?this.low>>>0:this.low},$.toNumber=function(){return this.unsigned?(this.high>>>0)*uP+(this.low>>>0):this.high*uP+(this.low>>>0)},$.toString=function(e){if(e||=10,e<2||36<e)throw RangeError(`radix`);if(this.isZero())return`0`;if(this.isNegative())if(this.eq(xP)){var t=iP(e),n=this.div(t),r=n.mul(t).sub(this);return n.toString(e)+r.toInt().toString(e)}else return`-`+this.neg().toString(e);for(var i=iP(aP(e,6),this.unsigned),a=this,o=``;;){var s=a.div(i),c=(a.sub(s.mul(i)).toInt()>>>0).toString(e);if(a=s,a.isZero())return c+o;for(;c.length<6;)c=`0`+c;o=``+c+o}},$.getHighBits=function(){return this.high},$.getHighBitsUnsigned=function(){return this.high>>>0},$.getLowBits=function(){return this.low},$.getLowBitsUnsigned=function(){return this.low>>>0},$.getNumBitsAbs=function(){if(this.isNegative())return this.eq(xP)?64:this.neg().getNumBitsAbs();for(var e=this.high==0?this.low:this.high,t=31;t>0&&!(e&1<<t);t--);return this.high==0?t+1:t+33},$.isSafeInteger=function(){var e=this.high>>21;return!e||!this.unsigned&&e===-1&&!(this.low===0&&this.high===-2097152)},$.isZero=function(){return this.high===0&&this.low===0},$.eqz=$.isZero,$.isNegative=function(){return!this.unsigned&&this.high<0},$.isPositive=function(){return this.unsigned||this.high>=0},$.isOdd=function(){return(this.low&1)==1},$.isEven=function(){return(this.low&1)==0},$.equals=function(e){return $N(e)||(e=sP(e)),this.unsigned!==e.unsigned&&this.high>>>31==1&&e.high>>>31==1?!1:this.high===e.high&&this.low===e.low},$.eq=$.equals,$.notEquals=function(e){return!this.eq(e)},$.neq=$.notEquals,$.ne=$.notEquals,$.lessThan=function(e){return this.comp(e)<0},$.lt=$.lessThan,$.lessThanOrEqual=function(e){return this.comp(e)<=0},$.lte=$.lessThanOrEqual,$.le=$.lessThanOrEqual,$.greaterThan=function(e){return this.comp(e)>0},$.gt=$.greaterThan,$.greaterThanOrEqual=function(e){return this.comp(e)>=0},$.gte=$.greaterThanOrEqual,$.ge=$.greaterThanOrEqual,$.compare=function(e){if($N(e)||(e=sP(e)),this.eq(e))return 0;var t=this.isNegative(),n=e.isNegative();return t&&!n?-1:!t&&n?1:this.unsigned?e.high>>>0>this.high>>>0||e.high===this.high&&e.low>>>0>this.low>>>0?-1:1:this.sub(e).isNegative()?-1:1},$.comp=$.compare,$.negate=function(){return!this.unsigned&&this.eq(xP)?xP:this.not().add(gP)},$.neg=$.negate,$.add=function(e){$N(e)||(e=sP(e));var t=this.high>>>16,n=this.high&65535,r=this.low>>>16,i=this.low&65535,a=e.high>>>16,o=e.high&65535,s=e.low>>>16,c=e.low&65535,l=0,u=0,d=0,f=0;return f+=i+c,d+=f>>>16,f&=65535,d+=r+s,u+=d>>>16,d&=65535,u+=n+o,l+=u>>>16,u&=65535,l+=t+a,l&=65535,Q(d<<16|f,l<<16|u,this.unsigned)},$.subtract=function(e){return $N(e)||(e=sP(e)),this.add(e.neg())},$.sub=$.subtract,$.multiply=function(e){if(this.isZero())return this;if($N(e)||(e=sP(e)),QN)return Q(QN.mul(this.low,this.high,e.low,e.high),QN.get_high(),this.unsigned);if(e.isZero())return this.unsigned?hP:mP;if(this.eq(xP))return e.isOdd()?xP:mP;if(e.eq(xP))return this.isOdd()?xP:mP;if(this.isNegative())return e.isNegative()?this.neg().mul(e.neg()):this.neg().mul(e).neg();if(e.isNegative())return this.mul(e.neg()).neg();if(this.lt(pP)&&e.lt(pP))return iP(this.toNumber()*e.toNumber(),this.unsigned);var t=this.high>>>16,n=this.high&65535,r=this.low>>>16,i=this.low&65535,a=e.high>>>16,o=e.high&65535,s=e.low>>>16,c=e.low&65535,l=0,u=0,d=0,f=0;return f+=i*c,d+=f>>>16,f&=65535,d+=r*c,u+=d>>>16,d&=65535,d+=i*s,u+=d>>>16,d&=65535,u+=n*c,l+=u>>>16,u&=65535,u+=r*s,l+=u>>>16,u&=65535,u+=i*o,l+=u>>>16,u&=65535,l+=t*c+n*s+r*o+i*a,l&=65535,Q(d<<16|f,l<<16|u,this.unsigned)},$.mul=$.multiply,$.divide=function(e){if($N(e)||(e=sP(e)),e.isZero())throw Error(`division by zero`);if(QN)return!this.unsigned&&this.high===-2147483648&&e.low===-1&&e.high===-1?this:Q((this.unsigned?QN.div_u:QN.div_s)(this.low,this.high,e.low,e.high),QN.get_high(),this.unsigned);if(this.isZero())return this.unsigned?hP:mP;var t,n,r;if(this.unsigned){if(e.unsigned||(e=e.toUnsigned()),e.gt(this))return hP;if(e.gt(this.shru(1)))return _P;r=hP}else{if(this.eq(xP))return e.eq(gP)||e.eq(vP)?xP:e.eq(xP)?gP:(t=this.shr(1).div(e).shl(1),t.eq(mP)?e.isNegative()?gP:vP:(n=this.sub(e.mul(t)),r=t.add(n.div(e)),r));if(e.eq(xP))return this.unsigned?hP:mP;if(this.isNegative())return e.isNegative()?this.neg().div(e.neg()):this.neg().div(e).neg();if(e.isNegative())return this.div(e.neg()).neg();r=mP}for(n=this;n.gte(e);){t=Math.max(1,Math.floor(n.toNumber()/e.toNumber()));for(var i=Math.ceil(Math.log(t)/Math.LN2),a=i<=48?1:aP(2,i-48),o=iP(t),s=o.mul(e);s.isNegative()||s.gt(n);)t-=a,o=iP(t,this.unsigned),s=o.mul(e);o.isZero()&&(o=gP),r=r.add(o),n=n.sub(s)}return r},$.div=$.divide,$.modulo=function(e){return $N(e)||(e=sP(e)),QN?Q((this.unsigned?QN.rem_u:QN.rem_s)(this.low,this.high,e.low,e.high),QN.get_high(),this.unsigned):this.sub(this.div(e).mul(e))},$.mod=$.modulo,$.rem=$.modulo,$.not=function(){return Q(~this.low,~this.high,this.unsigned)},$.countLeadingZeros=function(){return this.high?Math.clz32(this.high):Math.clz32(this.low)+32},$.clz=$.countLeadingZeros,$.countTrailingZeros=function(){return this.low?eP(this.low):eP(this.high)+32},$.ctz=$.countTrailingZeros,$.and=function(e){return $N(e)||(e=sP(e)),Q(this.low&e.low,this.high&e.high,this.unsigned)},$.or=function(e){return $N(e)||(e=sP(e)),Q(this.low|e.low,this.high|e.high,this.unsigned)},$.xor=function(e){return $N(e)||(e=sP(e)),Q(this.low^e.low,this.high^e.high,this.unsigned)},$.shiftLeft=function(e){return $N(e)&&(e=e.toInt()),(e&=63)==0?this:e<32?Q(this.low<<e,this.high<<e|this.low>>>32-e,this.unsigned):Q(0,this.low<<e-32,this.unsigned)},$.shl=$.shiftLeft,$.shiftRight=function(e){return $N(e)&&(e=e.toInt()),(e&=63)==0?this:e<32?Q(this.low>>>e|this.high<<32-e,this.high>>e,this.unsigned):Q(this.high>>e-32,this.high>=0?0:-1,this.unsigned)},$.shr=$.shiftRight,$.shiftRightUnsigned=function(e){return $N(e)&&(e=e.toInt()),(e&=63)==0?this:e<32?Q(this.low>>>e|this.high<<32-e,this.high>>>e,this.unsigned):Q(e===32?this.high:this.high>>>e-32,0,this.unsigned)},$.shru=$.shiftRightUnsigned,$.shr_u=$.shiftRightUnsigned,$.rotateLeft=function(e){var t;return $N(e)&&(e=e.toInt()),(e&=63)==0?this:e===32?Q(this.high,this.low,this.unsigned):e<32?(t=32-e,Q(this.low<<e|this.high>>>t,this.high<<e|this.low>>>t,this.unsigned)):(e-=32,t=32-e,Q(this.high<<e|this.low>>>t,this.low<<e|this.high>>>t,this.unsigned))},$.rotl=$.rotateLeft,$.rotateRight=function(e){var t;return $N(e)&&(e=e.toInt()),(e&=63)==0?this:e===32?Q(this.high,this.low,this.unsigned):e<32?(t=32-e,Q(this.high<<t|this.low>>>e,this.low<<t|this.high>>>e,this.unsigned)):(e-=32,t=32-e,Q(this.low<<t|this.high>>>e,this.high<<t|this.low>>>e,this.unsigned))},$.rotr=$.rotateRight,$.toSigned=function(){return this.unsigned?Q(this.low,this.high,!1):this},$.toUnsigned=function(){return this.unsigned?this:Q(this.low,this.high,!0)},$.toBytes=function(e){return e?this.toBytesLE():this.toBytesBE()},$.toBytesLE=function(){var e=this.high,t=this.low;return[t&255,t>>>8&255,t>>>16&255,t>>>24,e&255,e>>>8&255,e>>>16&255,e>>>24]},$.toBytesBE=function(){var e=this.high,t=this.low;return[e>>>24,e>>>16&255,e>>>8&255,e&255,t>>>24,t>>>16&255,t>>>8&255,t&255]},Z.fromBytes=function(e,t,n){return n?Z.fromBytesLE(e,t):Z.fromBytesBE(e,t)},Z.fromBytesLE=function(e,t){return new Z(e[0]|e[1]<<8|e[2]<<16|e[3]<<24,e[4]|e[5]<<8|e[6]<<16|e[7]<<24,t)},Z.fromBytesBE=function(e,t){return new Z(e[4]<<24|e[5]<<16|e[6]<<8|e[7],e[0]<<24|e[1]<<16|e[2]<<8|e[3],t)},typeof BigInt==`function`&&(Z.fromBigInt=function(e,t){return Q(Number(BigInt.asIntN(32,e)),Number(BigInt.asIntN(32,e>>BigInt(32))),t)},Z.fromValue=function(e,t){return typeof e==`bigint`?Z.fromBigInt(e,t):sP(e,t)},$.toBigInt=function(){var e=BigInt(this.low>>>0);return BigInt(this.unsigned?this.high>>>0:this.high)<<BigInt(32)|e});var SP=16;function CP(e){e===`X`&&(e=``);let t=e.padEnd(SP,`0`);return Z.fromString(t,!0,16)}function wP(e){if(e.isZero())return`X`;let t=e.countTrailingZeros(),n=t%4;t=(t-n)/4;let r=t;t*=4;let i=e.shiftRightUnsigned(t).toString(16).replace(/0+$/,``);return Array(17-r-i.length).join(`0`)+i}function TP(e,t){let n=EP(e).shiftRightUnsigned(2);return e.add(Z.fromNumber(2*t+1-4).multiply(n))}function EP(e){return e.and(e.not().add(1))}var DP=180/Math.PI;function OP(e){if(e.length===0)throw Error(`Invalid Hilbert quad key ${e}`);let t=e.split(`/`),n=parseInt(t[0],10),r=t[1],i=r.length,a=0,o=[0,0];for(let e=i-1;e>=0;e--){a=i-e;let t=r[e],n=0,s=0;t===`1`?s=1:t===`2`?(n=1,s=1):t===`3`&&(n=1);let c=2**(a-1);FP(c,o,n,s),o[0]+=c*n,o[1]+=c*s}if(n%2==1){let e=o[0];o[0]=o[1],o[1]=e}return{face:n,ij:o,level:a}}function kP(e){if(e.isZero())return``;let t=e.toString(2);for(;t.length<64;)t=`0`+t;let n=t.lastIndexOf(`1`),r=t.substring(0,3),i=t.substring(3,n),a=i.length/2,o=Z.fromString(r,!0,2).toString(10),s=``;if(a!==0)for(s=Z.fromString(i,!0,2).toString(4);s.length<a;)s=`0`+s;return`${o}/${s}`}function AP(e,t,n){let r=1<<t;return[(e[0]+n[0])/r,(e[1]+n[1])/r]}function jP(e){return e>=.5?1/3*(4*e*e-1):1/3*(1-4*(1-e)*(1-e))}function MP(e){return[jP(e[0]),jP(e[1])]}function NP(e,[t,n]){switch(e){case 0:return[1,t,n];case 1:return[-t,1,n];case 2:return[-t,-n,1];case 3:return[-1,-n,-t];case 4:return[n,-1,-t];case 5:return[n,t,-1];default:throw Error(`Invalid face`)}}function PP([e,t,n]){let r=Math.atan2(n,Math.sqrt(e*e+t*t));return[Math.atan2(t,e)*DP,r*DP]}function FP(e,t,n,r){if(r===0){n===1&&(t[0]=e-1-t[0],t[1]=e-1-t[1]);let r=t[0];t[0]=t[1],t[1]=r}}function IP(e){let t=MP(AP(e.ij,e.level,[.5,.5]));return PP(NP(e.face,t))}var LP=100;function RP(e){let{face:t,ij:n,level:r}=e,i=[[0,0],[0,1],[1,1],[1,0],[0,0]],a=Math.max(1,Math.ceil(LP*2**-r)),o=new Float64Array(4*a*2+2),s=0,c=0;for(let e=0;e<4;e++){let l=i[e].slice(0),u=i[e+1],d=(u[0]-l[0])/a,f=(u[1]-l[1])/a;for(let e=0;e<a;e++){l[0]+=d,l[1]+=f;let e=PP(NP(t,MP(AP(n,r,l))));Math.abs(e[1])>89.999&&(e[0]=c);let i=e[0]-c;e[0]+=i>180?-360:i<-180?360:0,o[s++]=e[0],o[s++]=e[1],c=e[0]}}return o[s++]=o[0],o[s++]=o[1],o}function zP(e){return OP(BP(e))}function BP(e){return e.indexOf(`/`)>0?e:kP(CP(e))}function VP(e){return IP(zP(e))}function HP(e){let t;if(e.face===2||e.face===5){let n=null,r=0;for(let t=0;t<4;t++){let i=RP(zP(`${e.face}/${t}`));n??=new Float64Array(4*i.length),n.set(i,r),r+=i.length}t=UP(n)}else t=UP(RP(e));return t}function UP(e){if(e.length%2!=0)throw Error(`Invalid corners`);let t=[],n=[];for(let r=0;r<e.length;r+=2)t.push(e[r]),n.push(e[r+1]);return t.sort((e,t)=>e-t),n.sort((e,t)=>e-t),{west:t[0],east:t[t.length-1],north:n[n.length-1],south:n[0]}}function WP(e,t){let n=t?.minimumHeight||0,r=t?.maximumHeight||0,i=HP(zP(e)),a=i.west,o=i.south,s=i.east,c=i.north,l=[];return l.push(new I(a,c,n)),l.push(new I(s,c,n)),l.push(new I(s,o,n)),l.push(new I(a,o,n)),l.push(new I(a,c,r)),l.push(new I(s,c,r)),l.push(new I(s,o,r)),l.push(new I(a,o,r)),l}function GP(e){let t=e.token,n={minimumHeight:e.minimumHeight,maximumHeight:e.maximumHeight},r=WP(t,n),i=VP(t),a=i[0],o=i[1],s=J.WGS84.cartographicToCartesian([a,o,n.maximumHeight]),c=new I(s[0],s[1],s[2]);r.push(c);let l=Jy(r);return[...l.center,...l.halfAxes]}var KP={QUADTREE:4,OCTREE:8};function qP(e,t,n){if(e?.box){let r=wP(TP(CP(e.s2VolumeInfo.token),t)),i={...e.s2VolumeInfo};switch(i.token=r,n){case`OCTREE`:let t=e.s2VolumeInfo,n=t.maximumHeight-t.minimumHeight,r=n/2,i=t.minimumHeight+n/2;t.minimumHeight=i-r,t.maximumHeight=i+r;break;default:break}return{box:GP(i),s2VolumeInfo:i}}}async function JP(e){let{subtree:t,subtreeData:n={level:0,x:0,y:0,z:0},parentData:r={mortonIndex:0,localLevel:-1,localX:0,localY:0,localZ:0},childIndex:i=0,implicitOptions:a,loaderOptions:o,s2VolumeBox:s}=e,{subdivisionScheme:c,subtreeLevels:l,maximumLevel:u,contentUrlTemplate:d,subtreesUriTemplate:f,basePath:p}=a,m={children:[],lodMetricValue:0,contentUrl:``};if(typeof u!=`number`||!Number.isFinite(u))return ge.once(`Missing 'maximumLevel' or 'availableLevels' property. The subtree ${d} won't be loaded...`),m;let h=r.localLevel+1,g=n.level+h;if(g>u)return m;let _=KP[c],v=Math.log2(_),y=i&1,b=i>>1&1,x=i>>2&1,S=QP(r.localX,y,1),C=QP(r.localY,b,1),w=QP(r.localZ,x,1),T=QP(n.x,S,h),E=QP(n.y,C,h),D=QP(n.z,w,h),O=QP(r.mortonIndex,i,v),k=h===l&&YP(t.childSubtreeAvailability,O),A,j,ee,te;if(k?(A=await sc($P(`${p}/${f}`,g,T,E,D),ZN,o),te=0,j={level:g,x:T,y:E,z:D},ee={mortonIndex:0,localLevel:0,localX:0,localY:0,localZ:0}):(A=t,te=(_**h-1)/(_-1)+O,j=n,ee={mortonIndex:O,localLevel:h,localX:S,localY:C,localZ:w}),!YP(A.tileAvailability,te))return m;YP(A.contentAvailability,te)&&(m.contentUrl=$P(d,g,T,E,D));for(let e=0;e<_;e++){let t=qP(s,e,c),n=await JP({subtree:A,subtreeData:j,parentData:ee,childIndex:e,implicitOptions:a,loaderOptions:o,s2VolumeBox:t});(n.contentUrl||n.children.length)&&m.children.push(n)}return m.contentUrl||m.children.length?XP(m,{level:g,x:T,y:E,z:D},a,s):m}function YP(e,t){let n;return Array.isArray(e)?(n=e[0],e.length>1&&ge.once(`Not supported extension "3DTILES_multiple_contents" has been detected`)):n=e,`constant`in n?!!n.constant:n.explicitBitstream?tF(t,n.explicitBitstream):!1}function XP(e,t,n,r){let{basePath:i,refine:a,getRefine:o,lodMetricType:s,getTileType:c,rootLodMetricValue:l,rootBoundingVolume:u}=n,d=e.contentUrl&&e.contentUrl.replace(`${i}/`,``),f=l/2**t.level,p=ZP(r?.box?{box:r.box}:u,t,n.subdivisionScheme);return{children:e.children,contentUrl:e.contentUrl,content:{uri:d},id:e.contentUrl,refine:o(a),type:c(e),lodMetricType:s,lodMetricValue:f,geometricError:f,transform:e.transform,boundingVolume:p}}function ZP(e,t,n){if(e.region){let{level:r,x:i,y:a,z:o}=t,[s,c,l,u,d,f]=e.region,p=2**r,m=(l-s)/p,[h,g]=[s+m*i,s+m*(i+1)],_=(u-c)/p,[v,y]=[c+_*a,c+_*(a+1)],b,x;if(n===`OCTREE`){let e=(f-d)/p;[b,x]=[d+e*o,d+e*(o+1)]}else[b,x]=[d,f];return{region:[h,v,g,y,b,x]}}if(e.box)return e;throw Error(`Unsupported bounding volume type ${JSON.stringify(e)}`)}function QP(e,t,n){return(e<<n)+t}function $P(e,t,n,r,i){let a=eF({level:t,x:n,y:r,z:i});return e.replace(/{level}|{x}|{y}|{z}/gi,e=>a[e])}function eF(e){let t={};for(let n in e)t[`{${n}}`]=e[n];return t}function tF(e,t){let n=Math.floor(e/8),r=e%8;return(t[n]>>r&1)==1}function nF(e,t=``){if(!t)return bj.EMPTY;let n=t.split(`?`)[0].split(`.`).pop();switch(n){case`pnts`:return bj.POINTCLOUD;case`i3dm`:case`b3dm`:case`glb`:case`gltf`:return bj.SCENEGRAPH;default:return n||bj.EMPTY}}function rF(e){switch(e){case`REPLACE`:case`replace`:return yj.REPLACE;case`ADD`:case`add`:return yj.ADD;default:return e}}function iF(e,t){if(/^[a-z][0-9a-z+.-]*:/i.test(t)){let n=new URL(e,`${t}/`);return decodeURI(n.toString())}else if(e.startsWith(`/`))return e;return AM(t,e)}function aF(e,t){if(!e)return null;let n;if(e.content){let r=e.content.uri||e.content?.url;r!==void 0&&(n=iF(r,t))}return{...e,id:n,contentUrl:n,lodMetricType:Sj.GEOMETRIC_ERROR,lodMetricValue:e.geometricError,transformMatrix:e.transform,type:nF(e,n),refine:rF(e.refine)}}async function oF(e,t,n){let r=null,i=lF(e.root);r=i&&e.root?await sF(e.root,e,t,i,n):aF(e.root,t);let a=[];for(a.push(r);a.length>0;){let r=a.pop()||{},i=r.children||[],o=[];for(let r of i){let i=lF(r),s;s=i?await sF(r,e,t,i,n):aF(r,t),s&&(o.push(s),a.push(s))}r.children=o}return r}async function sF(e,t,n,r,i){let{subdivisionScheme:a,maximumLevel:o,availableLevels:s,subtreeLevels:c,subtrees:{uri:l}}=r,u=await sc(iF($P(l,0,0,0,0),n),ZN,i),d=e.content?.uri,f=d?iF(d,n):``,p=t?.root?.refine,m=e.geometricError,h=e.boundingVolume.extensions?.[`3DTILES_bounding_volume_S2`];h&&(e.boundingVolume={box:GP(h),s2VolumeInfo:h});let g=e.boundingVolume;return await cF(e,n,u,{contentUrlTemplate:f,subtreesUriTemplate:l,subdivisionScheme:a,subtreeLevels:c,maximumLevel:Number.isFinite(s)?s-1:o,refine:p,basePath:n,lodMetricType:Sj.GEOMETRIC_ERROR,rootLodMetricValue:m,rootBoundingVolume:g,getTileType:nF,getRefine:rF},i)}async function cF(e,t,n,r,i){if(!e)return null;let{children:a,contentUrl:o}=await JP({subtree:n,implicitOptions:r,loaderOptions:i}),s,c=null;return o&&(s=o,c={uri:o.replace(`${t}/`,``)}),{...e,id:s,contentUrl:s,lodMetricType:Sj.GEOMETRIC_ERROR,lodMetricValue:e.geometricError,transformMatrix:e.transform,type:nF(e,s),refine:rF(e.refine),content:c||e.content,children:a}}function lF(e){return e?.extensions?.[`3DTILES_implicit_tiling`]||e?.implicitTiling}var uF={dataType:null,batchType:null,id:`3d-tiles`,name:`3D Tiles`,module:`3d-tiles`,version:PM,extensions:[`cmpt`,`pnts`,`b3dm`,`i3dm`],mimeTypes:[`application/octet-stream`],tests:[`cmpt`,`pnts`,`b3dm`,`i3dm`],parse:dF,options:{"3d-tiles":{loadGLTF:!0,decodeQuantizedPositions:!1,isTileset:`auto`,assetGltfUpAxis:null}}};async function dF(e,t={},n){let r=t[`3d-tiles`]||{},i;return i=r.isTileset===`auto`?n?.url&&n.url.indexOf(`.json`)!==-1:r.isTileset,i?fF(e,t,n):pF(e,t,n)}async function fF(e,t,n){let r=JSON.parse(new TextDecoder().decode(e)),i=n?.url||``,a=mF(i),o=await oF(r,a,t||{});return{...r,shape:`tileset3d`,loader:uF,url:i,queryString:n?.queryString||``,basePath:a,root:o||r.root,type:xj.TILES3D,lodMetricType:Sj.GEOMETRIC_ERROR,lodMetricValue:r.root?.geometricError||0}}async function pF(e,t,n){let r={content:{shape:`tile3d`,featureIds:null}};return await GN(e,0,t,n,r.content),r.content}function mF(e){return kM(e)}var hF=[0],gF={getPointColor:{type:`accessor`,value:[0,0,0,255]},pointSize:1,data:``,loader:uF,onTilesetLoad:{type:`function`,value:e=>{}},onTileLoad:{type:`function`,value:e=>{}},onTileUnload:{type:`function`,value:e=>{}},onTileError:{type:`function`,value:(e,t,n)=>{}},_getMeshColor:{type:`function`,value:e=>[255,255,255]}},_F=class extends rv{initializeState(){`onTileLoadFail`in this.props&&z.removed(`onTileLoadFail`,`onTileError`)(),this.state={layerMap:{},tileset3d:null,activeViewports:{},lastUpdatedViewports:null}}get isLoaded(){return!!(this.state?.tileset3d?.isLoaded()&&super.isLoaded)}shouldUpdateState({changeFlags:e}){return e.somethingChanged}updateState({props:e,oldProps:t,changeFlags:n}){if(e.data&&e.data!==t.data&&this._loadTileset(e.data),n.viewportChanged){let{activeViewports:e}=this.state;Object.keys(e).length&&(this._updateTileset(e),this.state.lastUpdatedViewports=e,this.state.activeViewports={})}if(n.propsChanged){let{layerMap:e}=this.state;for(let t in e)e[t].needsUpdate=!0}}finalizeState(e){this.state.tileset3d?.destroy(),this.state.tileset3d=null,this.state.layerMap={},this.state.activeViewports={},this.state.lastUpdatedViewports=null,super.finalizeState(e)}activateViewport(e){let{activeViewports:t,lastUpdatedViewports:n}=this.state;this.internalState.viewport=e,t[e.id]=e;let r=n?.[e.id];(!r||!e.equals(r))&&(this.setChangeFlags({viewportChanged:!0}),this.setNeedsUpdate())}getPickingInfo({info:e,sourceLayer:t}){let n=t&&t.props.tile;return e.picked&&(e.object=n),e.sourceTile=n,e}filterSubLayer({layer:e,viewport:t,cullRect:n,isPicking:r}){let{tile:i}=e.props,{id:a}=t;if(!i.selected||!i.viewportIds.includes(a))return!1;if(r&&n&&i.content?.cartographicOrigin){let[e,r]=t.project(i.content.cartographicOrigin),a=n.x+n.width/2,o=n.y+n.height/2,s=Math.max(t.width,t.height)/4,c=e-a,l=r-o;if(c*c+l*l>s*s)return!1}return!0}_updateAutoHighlight(e){let t=e.sourceTile,n=this.state.layerMap[t?.id];n&&n.layer&&n.layer.updateAutoHighlight(e)}async _loadTileset(e){let t=this.props.loadOptions||{},n=this.props.loaders?.length?this.props.loaders:this.props.loader,r=Array.isArray(n)?n[0]:n,{tileset:i,...a}=t,o={loadOptions:{...a},...i},s=e;if(`preload`in r&&typeof r.preload==`function`){let n=await r.preload(e,t);n.url&&(s=n.url),n.headers&&(o.loadOptions.core={...o.loadOptions.core,fetch:{...o.loadOptions.core?.fetch,headers:n.headers}}),Object.assign(o,n)}let c=new wM(await sc(s,r,o.loadOptions),{onTileLoad:this._onTileLoad.bind(this),onTileUnload:this._onTileUnload.bind(this),onTileError:this.props.onTileError,onUpdate:()=>this.setNeedsUpdate(),...o});this.setState({tileset3d:c,layerMap:{}}),this._updateTileset(this.state.activeViewports),this.props.onTilesetLoad(c)}_onTileLoad(e){let{lastUpdatedViewports:t}=this.state;e.tileDrawn=!1,this.props.onTileLoad(e),this._updateTileset(t),this.setNeedsUpdate()}_onTileUnload(e){delete this.state.layerMap[e.id],this.props.onTileUnload(e)}_updateTileset(e){if(!e)return;let{tileset3d:t}=this.state,{timeline:n}=this.context,r=Object.keys(e).length;!n||!r||!t||t.selectTiles(Object.values(e)).then(e=>{this.state.frameNumber!==e&&this.setState({frameNumber:e})})}_getSubLayer(e,t){if(!e.content)return null;switch(e.type){case bj.POINTCLOUD:return this._makePointCloudLayer(e,t);case bj.SCENEGRAPH:return this._make3DModelLayer(e);case bj.MESH:return this._makeSimpleMeshLayer(e,t);default:throw Error(`Tile3DLayer: Failed to render layer of type ${e.content.type}`)}}_makePointCloudLayer(e,t){let{attributes:n,pointCount:r,constantRGBA:i,cartographicOrigin:a,modelMatrix:o}=e.content,{positions:s,normals:c,colors:l}=n;if(!s)return null;let u=t&&t.props.data||{header:{vertexCount:r},attributes:{POSITION:s,NORMAL:c,COLOR_0:l}},{pointSize:d,getPointColor:f}=this.props;return new(this.getSubLayerClass(`pointcloud`,ly))({pointSize:d},this.getSubLayerProps({id:`pointcloud`}),{id:`${this.id}-pointcloud-${e.id}`,tile:e,data:u,coordinateSystem:ed.METER_OFFSETS,coordinateOrigin:a,modelMatrix:o,getColor:i||f,_offset:0})}_make3DModelLayer(e){let{gltf:t,instances:n,cartographicOrigin:r,modelMatrix:i}=e.content;return new(this.getSubLayerClass(`scenegraph`,rA))({_lighting:`pbr`},this.getSubLayerProps({id:`scenegraph`}),{id:`${this.id}-scenegraph-${e.id}`,tile:e,data:n||hF,scenegraph:t,coordinateSystem:ed.METER_OFFSETS,coordinateOrigin:r,modelMatrix:i,getTransformMatrix:e=>e.modelMatrix,getPosition:[0,0,0],_offset:0,onFirstDraw:()=>{e.tileDrawn=!0}})}_makeSimpleMeshLayer(e,t){let{attributes:n,indices:r,modelMatrix:i,cartographicOrigin:a,coordinateSystem:o=ed.METER_OFFSETS,material:s,featureIds:c}=e.content,{_getMeshColor:l}=this.props,u=t&&t.props.mesh||new de({topology:`triangle-list`,attributes:vF(n),indices:r});return new(this.getSubLayerClass(`mesh`,pA))(this.getSubLayerProps({id:`mesh`}),{id:`${this.id}-mesh-${e.id}`,tile:e,mesh:u,data:hF,getColor:l(e),pbrMaterial:s,modelMatrix:i,coordinateOrigin:a,coordinateSystem:o,featureIds:c,_offset:0})}renderLayers(){let{tileset3d:e,layerMap:t}=this.state;return e?e.tiles.map(e=>{let n=t[e.id]=t[e.id]||{tile:e},{layer:r}=n;return e.selected&&(r?n.needsUpdate&&=(r=this._getSubLayer(e,r),!1):r=this._getSubLayer(e)),n.layer=r,r}).filter(Boolean):null}};_F.defaultProps=gF,_F.layerName=`Tile3DLayer`;function vF(e){let t={};return t.positions={...e.positions,value:new Float32Array(e.positions.value)},e.normals&&(t.normals=e.normals),e.texCoords&&(t.texCoords=e.texCoords),e.colors&&(t.colors=e.colors),e.uvRegions&&(t.uvRegions=e.uvRegions),t}function yF(e,t,n=fetch){let r=new Set(e),i=new Set;return{async fetch(e,a){i.delete(e);let o=await n(e,{...a,signal:t});return i.delete(e),!t.aborted&&r.has(e)&&o.status===204&&i.add(e),o},async contentLoader(e){!t.aborted&&e.content===null&&i.has(e.contentUrl)&&(e.hasEmptyContent=!0,e.content={byteLength:0,gpuMemoryUsageInBytes:0})}}}var bF=85.0511287798066,xF=156543.03392804097,SF=6378137,CF=.00669437999014;function wF(e){throw Error(`Invalid 3D tileset: ${e}`)}function TF(e,t){return(typeof e!=`number`||!Number.isFinite(e))&&wF(`${t} must be finite`),e}function EF(e,t){return(!Array.isArray(e)||e.length!==4||e.some(e=>!Number.isFinite(e))||e[0]>e[2]||e[1]>e[3])&&wF(`${t} is invalid`),e}function DF(e,t){let n=[Math.max(e[0],t[0]),Math.max(e[1],t[1]),Math.min(e[2],t[2]),Math.min(e[3],t[3])];return n[0]<=n[2]&&n[1]<=n[3]?n:null}function OF(e){return Math.max(-85.0511287798066,Math.min(bF,e))}function kF(e,t){let n=2**t;return Math.max(0,Math.min(n-1,Math.floor((e+180)/360*n)))}function AF(e,t){let n=2**t,r=OF(e)*Math.PI/180,i=(1-Math.asinh(Math.tan(r))/Math.PI)/2;return Math.max(0,Math.min(n-1,Math.floor(i*n)))}function jF(e,t,n,r,i){let a=2**e,o=(t/a*2-1)*Math.PI,s=((t+1)/a*2-1)*Math.PI,c=Math.atan(Math.sinh(Math.PI*(1-2*n/a)));return[o,Math.atan(Math.sinh(Math.PI*(1-2*(n+1)/a))),s,c,r,i]}function MF(e,t,n){let r=2**e,i=((t+.5)/r*2-1)*Math.PI,a=Math.atan(Math.sinh(Math.PI*(1-2*(n+.5)/r))),o=Math.sin(i),s=Math.cos(i),c=Math.sin(a),l=Math.cos(a),u=SF/Math.sqrt(1-CF*c**2),d=SF*(1-CF)/(1-CF*c**2)**1.5,f=2*Math.PI/r,p=u*l*f,m=d*l*f,h=[-o,s,0],g=[c*s,c*o,-l],_=[l*s,l*o,c],v=[u*l*s,u*l*o,u*(1-CF)*c],y=h.map(e=>e*p),b=g.map(e=>e*m),x=v.map((e,t)=>e-y[t]/2-b[t]/2);return[y[0],y[1],y[2],0,b[0],b[1],b[2],0,_[0],_[1],_[2],0,x[0],x[1],x[2],1]}function NF(e,t){typeof e!=`string`&&wF(`root content URI is missing`);let n=new URL(e,`${t.toString().replace(/\/$/,``)}/`),r=n.pathname.replace(/%7B/gi,`{`).replace(/%7D/gi,`}`);return(n.protocol!==t.protocol||n.origin!==t.origin||n.username||n.password||n.search||n.hash||r!==`${t.pathname.replace(/\/$/,``)}/tiles/{level}/{y}/{x}.glb`)&&wF(`root content URI must be its same-origin level/row/column GLB template`),n.toString().replace(/%7B/gi,`{`).replace(/%7D/gi,`}`)}function PF(e,t){let n=TF(e.geometricError,`root geometricError`);n<=0&&wF(`root geometricError must be positive`);let r=Math.log2(xF/n),i=Math.round(r);return(i<0||i>30||Math.abs(r-i)>1e-6||i+t-1>30)&&wF(`root geometricError does not identify a supported WebMercatorQuad range`),[i,i+t-1]}function FF(e){let t=e.tileset;(!t||typeof t!=`object`||!t.root||t.asset?.version!==`1.1`)&&wF(`asset 1.1 and root are required`);let n=t.root,r=n.implicitTiling;(!r||r.subdivisionScheme!==`QUADTREE`)&&wF(`a QUADTREE implicit root is required`);let i=TF(r.availableLevels,`availableLevels`);(!Number.isSafeInteger(i)||i<1)&&wF(`availableLevels must be a positive integer`);let a=n.boundingVolume?.region;(!Array.isArray(a)||a.length!==6||a.some(e=>typeof e!=`number`||!Number.isFinite(e)))&&wF(`root bounding region is invalid`);let o=a[4],s=a[5];o>s&&wF(`root height range is invalid`);let c=new URL(e.pageUrl),l=new URL(e.rootUrl,c);(![`http:`,`https:`].includes(l.protocol)||l.origin!==c.origin)&&wF(`root URL must use the page origin`);let u=NF(n.content?.uri??n.content?.url,l),[d,f]=PF(n,i),p=Math.floor(TF(e.requestedZoom,`requested zoom`)),m=e.maxTiles??64;(!Number.isSafeInteger(m)||m<1)&&wF(`tile budget must be a positive integer`);let h=Math.max(d,14);if(f<h&&wF(`advertised zoom range is below the supported placement precision`),p<h)return{kind:`too-coarse`,requestedZoom:p,minimumZoom:h,tileCount:0,maxTiles:m};let g=Math.min(f,p),_=DF(EF(e.collectionExtent,`collection extent`),EF(e.viewport,`viewport`));if(!_)return{kind:`empty`,zoom:g,tileCount:0,maxTiles:m};let v=kF(_[0],g),y=kF(_[2],g),b=AF(_[3],g),x=AF(_[1],g),S=(y-v+1)*(x-b+1);if(S>m)return{kind:`too-many`,zoom:g,tileCount:S,maxTiles:m};let C=[];for(let e=b;e<=x;e+=1)for(let t=v;t<=y;t+=1)C.push({boundingVolume:{region:jF(g,t,e,o,s)},content:{uri:u.replace(`{level}`,String(g)).replace(`{y}`,String(e)).replace(`{x}`,String(t))},geometricError:0,transform:MF(g,t,e)});let[w,T,E,D]=_,O=TF(t.geometricError??n.geometricError,`geometricError`);return{kind:`manifest`,maxTiles:m,tileCount:S,zoom:g,tileset:{asset:{version:`1.1`,gltfUpAxis:`Z`},geometricError:O,root:{boundingVolume:{region:[w*Math.PI/180,T*Math.PI/180,E*Math.PI/180,D*Math.PI/180,o,s]},children:C,geometricError:O,refine:`REPLACE`}}}}var IF=`tellurion-3d-places`,LF=class extends HTMLElement{#e=null;#t=null;#n;#r;#i=[];#a=null;#o=null;#s=0;#c=()=>{this.#m(!0)};#l=()=>{this.#m(!1)};#u=()=>{this.#m(!0)};connectedCallback(){this.innerHTML=`
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
      </div>
      <p class="panel__status" data-field="status"></p>
      <div class="panel__map" data-field="map"></div>
    `,this.#n=this.querySelector(`[data-field="collection"]`),this.#r=this.querySelector(`[data-field="status"]`);let e=this.querySelector(`[data-field="map"]`);this.#e=a(e),this.#e.setPitch(45),this.#t=new $v({interleaved:!0,layers:[]}),this.#e.addControl(this.#t),this.#e.addControl(new i,`top-right`),this.#e.on(`load`,this.#c),this.#e.on(`moveend`,this.#l),this.#n.addEventListener(`change`,this.#u),this.#d()}disconnectedCallback(){this.#s+=1,this.#a?.abort(),this.#a=null,this.#f(),this.#n?.removeEventListener(`change`,this.#u),this.#e?.off(`load`,this.#c),this.#e?.off(`moveend`,this.#l),this.#e?.remove(),this.#e=null,this.#t=null}async#d(){try{let e=await t();if(this.#i=e.collections,this.#n.replaceChildren(...this.#i.map(e=>{let t=document.createElement(`option`);return t.value=e.id,t.textContent=e.id,t})),this.#i.length===0){this.#r.textContent=`no collections available`;return}this.#e?.loaded()&&this.#m(!0)}catch(e){this.#r.textContent=e instanceof Error?e.message:String(e)}}#f(){this.#o&&=(URL.revokeObjectURL(this.#o),null)}#p(){this.#t?.setProps({layers:[]}),this.#f()}async#m(e){let t=this.#e,i=this.#t;if(!t||!i)return;let a=this.#i.find(e=>e.id===this.#n.value);if(!a)return;e&&s(t,a.extent),this.#r.textContent=`loading 3D tiles for "${a.id}"…`;let o=this.#s+1;this.#s=o,this.#a?.abort();let c=new AbortController;this.#a=c,this.#p();let l=`/${n}/3dtiles/catalogs/${r}/collections/${encodeURIComponent(a.id)}/3dtiles`;try{let e=await fetch(l,{headers:{Accept:`application/json`},signal:c.signal});if(!e.ok)throw Error(`3D tileset request failed: ${e.status} ${e.statusText}`);let n=await e.json();if(o!==this.#s||c.signal.aborted)return;let r=n&&typeof n==`object`?n.root:void 0,s=n&&typeof n==`object`?n.asset?.version:void 0;if(!r||typeof r!=`object`||s!==`1.1`)throw Error(`Invalid 3D tileset: asset 1.1 and root are required`);let u=l,d;if(`implicitTiling`in r){let e=a.extent?.spatial.bbox[0];if(!e)throw Error(`Invalid 3D tileset: collection extent is required`);let r=t.getBounds(),i=FF({tileset:n,rootUrl:l,pageUrl:window.location.href,collectionExtent:e,viewport:[r.getWest(),r.getSouth(),r.getEast(),r.getNorth()],requestedZoom:t.getZoom()});if(i.kind===`too-many`){this.#r.textContent=`zoom closer to view at most ${i.maxTiles} 3D tiles (${i.tileCount} requested)`;return}if(i.kind===`too-coarse`){this.#r.textContent=`zoom closer to level ${i.minimumZoom} for accurate 3D placement`;return}if(i.kind===`empty`){this.#r.textContent=`no 3D tiles in view for "${a.id}"`;return}let o=new Blob([JSON.stringify(i.tileset)],{type:`application/json`});d=yF(i.tileset.root.children.map(e=>e.content.uri),c.signal),u=URL.createObjectURL(o),this.#o=u}if(o!==this.#s||c.signal.aborted){u.startsWith(`blob:`)&&URL.revokeObjectURL(u);return}let f=new _F({id:IF,data:u,loadOptions:{"3d-tiles":{isTileset:!0},...d?{core:{fetch:d.fetch},tileset:{contentLoader:d.contentLoader}}:{}},onTilesetLoad:()=>{o===this.#s&&(this.#r.textContent=`serving 3D places for "${a.id}"`)},onTileError:(e,t)=>{o===this.#s&&(this.#r.textContent=`3D tile failed to load: ${t}`)}});i.setProps({layers:[f]})}catch(e){if(o!==this.#s||c.signal.aborted)return;this.#p(),this.#r.textContent=e instanceof Error?e.message:String(e)}}};customElements.define(`tellurion-places3d-panel`,LF);export{LF as TellurionPlaces3dPanel};