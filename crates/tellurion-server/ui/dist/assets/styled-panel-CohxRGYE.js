import{c as e,o as t,v as n,x as r}from"./index-CEU2jNJg.js";import{a as i,c as a,l as o,n as s,t as c}from"./api-DcdVQm8X.js";r();var l=`tellurion-styled`,u=class extends HTMLElement{#e=null;#t;#n;#r;#i;#a=[];#o=[];connectedCallback(){this.innerHTML=`
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
        <label>
          Style
          <select data-field="style"></select>
        </label>
      </div>
      <p class="panel__status" data-field="status"></p>
      <p class="panel__style-name" data-field="style-name"></p>
      <div class="panel__map" data-field="map"></div>
    `,this.#t=this.querySelector(`[data-field="collection"]`),this.#n=this.querySelector(`[data-field="style"]`),this.#r=this.querySelector(`[data-field="status"]`),this.#i=this.querySelector(`[data-field="style-name"]`);let e=this.querySelector(`[data-field="map"]`);this.#e=t(e),this.#e.on(`load`,()=>void this.#c()),this.#t.addEventListener(`change`,()=>void this.#c()),this.#n.addEventListener(`change`,()=>void this.#c()),this.#s()}disconnectedCallback(){this.#e?.remove(),this.#e=null}async#s(){try{let[e,t]=await Promise.all([i(),o()]);if(this.#a=e.collections,this.#o=t.styles.map(e=>e.id),this.#t.replaceChildren(...this.#a.map(e=>{let t=document.createElement(`option`);return t.value=e.id,t.textContent=e.id,t})),this.#n.replaceChildren(...this.#o.map(e=>{let t=document.createElement(`option`);return t.value=e,t.textContent=e,t})),this.#a.length===0||this.#o.length===0){this.#r.textContent=`no collections or no registered styles available`;return}this.#e?.loaded()&&await this.#c()}catch(e){this.#r.textContent=e instanceof Error?e.message:String(e)}}async#c(){let t=this.#e;if(!t)return;let r=this.#a.find(e=>e.id===this.#t.value),i=this.#n.value;if(!(!r||!i))try{let o=await a(i);this.#i.textContent=typeof o.name==`string`?`style: ${o.name}`:`style: ${i}`,t.getLayer(`styled-raster`)&&t.removeLayer(`styled-raster`),t.getSource(l)&&t.removeSource(l),t.addSource(l,{type:`raster`,tiles:[n(``,s,c,r.id,i)],tileSize:256}),t.addLayer({id:`styled-raster`,type:`raster`,source:l}),e(t,r.extent),this.#r.textContent=`serving "${i}"-styled tiles for "${r.id}"`}catch(e){this.#r.textContent=e instanceof Error?e.message:String(e)}}};customElements.define(`tellurion-styled-panel`,u);export{u as TellurionStyledPanel};