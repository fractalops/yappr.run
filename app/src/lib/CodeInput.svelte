<script lang="ts">
  import {scan} from '$lib/scan';
  import {decodeQr} from '$lib/qr';
  let {oncode,disabled=false,label='Connection code',imageLabel='Import a connection QR image'}:{oncode:(value:string)=>void|Promise<void>;disabled?:boolean;label?:string;imageLabel?:string}=$props();
  let paste=$state(false),code=$state(''),failure=$state(''),working=$state(false),input=$state<HTMLInputElement>();
  async function submit(value:string){failure='';working=true;try{await oncode(value.trim());}catch(e){failure=String(e);}finally{working=false;}}
  async function camera(){try{await submit((await scan()).content);}catch(e){if(String(e)!=='cancelled')failure=String(e);}}
  async function image(event:Event){
    const field=event.target as HTMLInputElement,file=field.files?.[0];if(!file)return;
    const url=URL.createObjectURL(file);failure='';working=true;
    try{
      const image=new Image();image.src=url;await image.decode();
      const ratio=Math.min(1,2400/Math.max(image.naturalWidth,image.naturalHeight));
      const canvas=document.createElement('canvas');canvas.width=Math.round(image.naturalWidth*ratio);canvas.height=Math.round(image.naturalHeight*ratio);
      const context=canvas.getContext('2d');if(!context)throw new Error('The image reader is unavailable.');
      context.drawImage(image,0,0,canvas.width,canvas.height);
      const pixels=context.getImageData(0,0,canvas.width,canvas.height),value=decodeQr(pixels.data,pixels.width,pixels.height);
      if(!value)throw new Error('The image has no readable QR code.');await submit(value);
    }catch(e){failure=String(e);}finally{URL.revokeObjectURL(url);field.value='';working=false;}
  }
  const id=$props.id();
</script>
<div class="code-input">
  <div class="actions"><button onclick={camera} disabled={disabled||working}>Scan QR</button><button onclick={()=>input?.click()} disabled={disabled||working}>Import QR image</button><button onclick={()=>paste=!paste} disabled={disabled||working} aria-expanded={paste} aria-controls={id}>Paste a code</button></div>
  <input class="file" type="file" accept="image/*" bind:this={input} onchange={image} aria-label={imageLabel}/>
  {#if paste}<form id={id} onsubmit={e=>{e.preventDefault();void submit(code);}}><label for={`${id}-code`}>{label}</label><textarea id={`${id}-code`} bind:value={code} rows="2" spellcheck="false" autocapitalize="off"></textarea><button class="confirm" disabled={disabled||working||!code.trim()}>Continue</button></form>{/if}
  {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
</div>
<style>
.actions{display:flex;flex-wrap:wrap;gap:6px 16px;margin:12px 0;}button{min-height:36px;padding:6px 0;border:0;background:transparent;color:#c1c7d0;font-size:13px;}button:hover{color:#edf0f5;}label{display:block;font-size:12px;color:var(--muted);margin:12px 0 6px;}textarea{width:100%;padding:10px;border:1px solid #3a3f49;border-radius:7px;background:#121418;color:inherit;resize:vertical;font-size:14px;overflow-wrap:anywhere;}.confirm{border:1px solid #4a4f5a;border-radius:6px;padding:6px 12px;margin-top:8px;}.file{display:none;}.failure{font-size:12px;color:#e0aaaa;overflow-wrap:anywhere;}
</style>
