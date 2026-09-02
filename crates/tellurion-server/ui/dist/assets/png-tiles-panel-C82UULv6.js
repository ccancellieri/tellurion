import{c as e,o as t,x as n,y as r}from"./index-G0SipTln.js";import{a as i,n as a,t as o}from"./api-DcdVQm8X.js";n();var s=`tellurion-png`,c=class extends HTMLElement{#e=null;#t;#n;#r=[];connectedCallback(){this.innerHTML=`
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
      </div>
      <p class="panel__status" data-field="status"></p>
      <div class="panel__map" data-field="map"></div>
    `,this.#t=this.querySelector(`[data-field="collection"]`),this.#n=this.querySelector(`[data-field="status"]`);let e=this.querySelector(`[data-field="map"]`);this.#e=t(e),this.#e.on(`load`,()=>this.#a()),this.#t.addEventListener(`change`,()=>this.#a()),this.#i()}disconnectedCallback(){this.#e?.remove(),this.#e=null}async#i(){try{let e=await i();if(this.#r=e.collections,this.#t.replaceChildren(...this.#r.map(e=>{let t=document.createElement(`option`);return t.value=e.id,t.textContent=e.id,t})),this.#r.length===0){this.#n.textContent=`no collections available`;return}this.#e?.loaded()&&this.#a()}catch(e){this.#n.textContent=e instanceof Error?e.message:String(e)}}#a(){let t=this.#e;if(!t)return;let n=this.#r.find(e=>e.id===this.#t.value);n&&(t.getLayer(`png-raster`)&&t.removeLayer(`png-raster`),t.getSource(s)&&t.removeSource(s),t.addSource(s,{type:`raster`,tiles:[r(``,a,o,n.id,`png`)],tileSize:256}),t.addLayer({id:`png-raster`,type:`raster`,source:s}),e(t,n.extent),this.#n.textContent=`serving PNG tiles for "${n.id}"`)}};customElements.define(`tellurion-png-panel`,c);export{c as TellurionPngPanel};