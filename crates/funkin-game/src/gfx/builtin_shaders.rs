//! Fragment sources of the engine's own shader classes (from the original
//! game's `funkin.graphics.shaders`, Apache-2.0), for the ones scripts create.


/// `DropShadowShader`.
pub const DROP_SHADOW_SHADER: &str = r#"
      #pragma header

      #ifdef GL_ES
        #ifdef GL_OES_standard_derivatives
          #define HAS_DERIVATIVES
        #endif
      #else
        #if __VERSION__ >= 130
          #define HAS_DERIVATIVES
        #endif
      #endif

      // This shader aims to mostly recreate how Adobe Animate/Flash handles drop shadows, but its main use here is for rim lighting.

      // this shader also includes a recreation of the Animate/Flash "Adjust Color" filter,
      // which was kindly provided and written by Rozebud https://github.com/ThatRozebudDude ( thank u rozebud :) )
      // Adapted from Andrey-Postelzhuks shader found here: https://forum.unity.com/threads/hue-saturation-brightness-contrast-shader.260649/
      // Hue rotation stuff is from here: https://www.w3.org/TR/filter-effects/#feColorMatrixElement

      // equals (frame.left, frame.top, frame.right, frame.bottom)
      uniform vec4 uFrameBounds;

      uniform float dist;
      uniform float str;
      uniform float thr;

      uniform float angCos;
      uniform float angSin;

      uniform sampler2D altMask;
      uniform bool useMask;
      uniform float thr2;

      uniform vec3 dropColor;

      uniform mat3 hueMatrix;
      uniform float contrast;
      uniform mat3 saturationMatrix;
      uniform float brightness;

      const vec3 lumaValue = vec3(0.2126, 0.7152, 0.0722);

      vec3 applyHSBCEffect(vec3 color)
      {
        vec3 bh = (brightness + color) * hueMatrix;
        vec3 c = (bh - 0.25) * contrast + 0.25;
        vec3 s = c * saturationMatrix;

        return s;
      }

      float getLumaRGB(vec3 color)
      {
        return dot(color.rgb, lumaValue);
      }

      vec4 getTexRGBA(vec2 uv)
      {
        return texture2D(bitmap, uv);
      }

      float getLumaTex(vec2 uv)
      {
        return getLumaRGB(getTexRGBA(uv).rgb);
      }

      // Used as Difference Based AA.
      // Return value range (0.0, 2.0)
      float lwidth_manual(float center, vec2 uv, vec2 px)
      {
        vec3 p2x1 = getTexRGBA(uv + vec2( 1.0,  0.0) * px).rgb;
        vec3 p1x2 = getTexRGBA(uv + vec2( 0.0,  1.0) * px).rgb;
        vec3 p2x2 = getTexRGBA(uv + vec2( 1.0,  1.0) * px).rgb;

        float right     = getLumaRGB(p2x1);
        float down      = getLumaRGB(p1x2);
        float diagonal  = getLumaRGB(p2x2);

        float dx = abs(right - center);
        float dy = abs(down - center);
        float dd = abs(diagonal - center);

        return ((dx + dy + dd * 0.7) / (1.0 + 1.0 + 0.7)) * 2.0;
      }

      // ============================
      // THRESHOLD
      // ============================

      float getThreshold(vec2 uv)
      {
        float threshold = thr;
        float maskIntensity = 0.0;

        if (useMask)
        {
          maskIntensity = texture2D(altMask, uv).b;
          if (maskIntensity > 0.0)
            threshold = thr2;
        }

        return threshold;
      }

      // ============================
      // DROP SHADOW / RIM
      // ============================

      vec4 createDropShadowEx(vec2 uv, vec2 ratio, vec2 size)
      {
        vec4 color4 = texture2D(bitmap, uv);

        #ifdef HAS_DERIVATIVES
          // Increase the pixel distance if the screen is smaller than the sprite!
          vec2 px = max(ratio, fwidth(uv));
        #else
          vec2 px = ratio;
        #endif

        float color3_light = getLumaRGB(color4.rgb);

        float delta = lwidth_manual(color3_light, uv, px);

        float threshold = getThreshold(uv);
        float intensity = smoothstep(threshold - delta, threshold + delta, color3_light);

        float shadowAlpha = 0.0;

        vec3 color3_no_effect = color4.a > 0.0 ? color4.rgb / color4.a : color4.rgb;
        vec3 color3 = applyHSBCEffect(color3_no_effect);

        vec2 checked = vec2(
          uv.x + (dist * angCos * ratio.x),
          uv.y - (dist * angSin * ratio.y)
        );

        if (checked.x > uFrameBounds.x &&
            checked.y > uFrameBounds.y &&
            checked.x < uFrameBounds.z &&
            checked.y < uFrameBounds.w)
        {
          shadowAlpha = texture2D(bitmap, checked).a;
        }

        float rim = (1.0 - (shadowAlpha * str)) * intensity;

        color3 += dropColor * rim;

        return vec4(color3 * color4.a, color4.a);
      }

      void main()
      {
        gl_FragColor = createDropShadowEx(openfl_TextureCoordv, 1.0 / openfl_TextureSize.xy, openfl_TextureSize.xy);
      }
    "#;

/// `SserafimShader`.
pub const SSERAFIM_SHADER: &str = r#"
      #pragma header

      // this shader includes a recreation of the Animate/Flash "Adjust Color" filter,
      // which was kindly provided and written by Rozebud https://github.com/ThatRozebudDude ( thank u rozebud :) )
      // Adapted from Andrey-Postelzhuks shader found here: https://forum.unity.com/threads/hue-saturation-brightness-contrast-shader.260649/
      // Hue rotation stuff is from here: https://www.w3.org/TR/filter-effects/#feColorMatrixElement

      uniform float hue;
      uniform float saturation;
      uniform float brightness;
      uniform float contrast;

      uniform float darkAmt;
      uniform vec3 lightColor;
      uniform float pulseStrength;
      uniform float truckStrength;
      uniform bool isChar;

      const vec3 grayscaleValues = vec3(0.3098039215686275, 0.607843137254902, 0.0823529411764706);
		  const float e = 2.718281828459045;

		  vec3 applyHueRotate(vec3 aColor, float aHue){
			  float angle = radians(aHue);

			  mat3 m1 = mat3(0.213, 0.213, 0.213, 0.715, 0.715, 0.715, 0.072, 0.072, 0.072);
			  mat3 m2 = mat3(0.787, -0.213, -0.213, -0.715, 0.285, -0.715, -0.072, -0.072, 0.928);
			  mat3 m3 = mat3(-0.213, 0.143, -0.787, -0.715, 0.140, 0.715, 0.928, -0.283, 0.072);
			  mat3 m = m1 + cos(angle) * m2 + sin(angle) * m3;

			  return m * aColor;
		  }

		  vec3 applySaturation(vec3 aColor, float value){
			  if(value > 0.0){ value = value * 3.0; }
			  value = (1.0 + (value / 100.0));
			  vec3 grayscale = vec3(dot(aColor, grayscaleValues));
        return clamp(mix(grayscale, aColor, value), 0.0, 1.0);
		  }

		  vec3 applyContrast(vec3 aColor, float value){
			  value = (1.0 + (value / 100.0));
			  if(value > 1.0){
				  value = (((0.00852259 * pow(e, 4.76454 * (value - 1.0))) * 1.01) - 0.0086078159) * 10.0; //Just roll with it...
				  value += 1.0;
			  }
        return clamp((aColor - 0.25) * value + 0.25, 0.0, 1.0);
		  }

      vec3 applyHSBCEffect(vec3 color){

			  //Brightness
			  color = color + ((brightness) / 255.0);

			  //Hue
			  color = applyHueRotate(color, hue);

			  //Contrast
			  color = applyContrast(color, contrast);

			  //Saturation
        color = applySaturation(color, saturation);

        return color;
      }

      #define saturate(v) clamp(v,0.,1.)

      vec3 hue2rgb(float hue){
	      hue=fract(hue);
	      return saturate(vec3(
		      abs(hue*6.-3.)-1.,
		      2.-abs(hue*6.-2.),
		      2.-abs(hue*6.-4.)
	      ));
      }

      vec3 rgb2hsl(vec3 c){
	      float cMin=min(min(c.r,c.g),c.b),
	      cMax=max(max(c.r,c.g),c.b),
	      delta=cMax-cMin;
	      vec3 hsl=vec3(0.,0.,(cMax+cMin)/2.);
	      if(delta!=0.0){ //If it has chroma and isnt gray.
		      if(hsl.z<.5){
			      hsl.y=delta/(cMax+cMin); //Saturation.
		      }else{
			      hsl.y=delta/(2.-cMax-cMin); //Saturation.
		    }
		    float deltaR=(((cMax-c.r)/6.)+(delta/2.))/delta,
		      deltaG=(((cMax-c.g)/6.)+(delta/2.))/delta,
		      deltaB=(((cMax-c.b)/6.)+(delta/2.))/delta;
		    //Hue.
		    if(c.r==cMax){
			    hsl.x=deltaB-deltaG;
		    }else if(c.g==cMax){
			    hsl.x=(1./3.)+deltaR-deltaB;
		    }else{ //if(c.b==cMax){
			    hsl.x=(2./3.)+deltaG-deltaR;
		    }
		      hsl.x=fract(hsl.x);
	      }
	      return hsl;
      }

      vec3 hsl2rgb(vec3 hsl){
	      if(hsl.y==0.){
		      return vec3(hsl.z); //Luminance.
	      }else{
		      float b;
		      if(hsl.z<.5){
			      b=hsl.z*(1.+hsl.y);
		      }else{
			      b=hsl.z+hsl.y-hsl.y*hsl.z;
		      }
		      float a=2.*hsl.z-b;
		      return a+hue2rgb(hsl.x)*(b-a);
	      }
      }

      // controls how much to brighten/darken depending on combinedAlpha.
      const float lightMultiplier = 0.07;

      void main()
      {
        vec4 col = texture2D(bitmap, openfl_TextureCoordv);

        vec3 unpremultipliedColor = col.a > 0.0 ? col.rgb / col.a : col.rgb;

        vec3 outColor = applyHSBCEffect(unpremultipliedColor);

        // truckLight1.alpha + backLightColor.alpha
        float combinedAlpha = truckStrength + pulseStrength;

        float darkFactor = (combinedAlpha*lightMultiplier) + darkAmt;
        if(darkAmt > 0.65) darkFactor = (combinedAlpha*(-1.0 * lightMultiplier)) + darkAmt;

        // interpolate between white and black depending on darkness
        vec3 multiplyColor;

        vec3 bruhColor = lightColor;
        bruhColor = rgb2hsl(bruhColor);
        bruhColor.b = bruhColor.b * pulseStrength;
        bruhColor = hsl2rgb(bruhColor);

        if(isChar == true){
          multiplyColor = mix(vec3(1.0, 1.0, 1.0), vec3(0.0, 0.0, 0.0), darkFactor/5.0);

          //multiplyColor = mix(multiplyColor, lightColor * vec3(pulseStrength), darkFactor/3.0);

          multiplyColor = mix(multiplyColor, bruhColor, darkFactor/3.0);
        }else{
          multiplyColor = mix(vec3(1.0, 1.0, 1.0), vec3(0.0, 0.0, 0.0), darkFactor);

          //multiplyColor = mix(multiplyColor, lightColor * vec3(pulseStrength), darkFactor/2.0);

          multiplyColor = mix(multiplyColor, bruhColor, darkFactor/2.0);
        }

        outColor *= multiplyColor;

        gl_FragColor = vec4(outColor.rgb * col.a, col.a);
      }


    "#;

/// `OverlayBlend`.
pub const OVERLAY_BLEND_SHADER: &str = r#"
        #pragma header

        uniform float alphaShit;
		uniform float yPos;
		uniform float xPos;

		uniform sampler2D funnyShit;


		vec4 blendOverlay(vec4 base, vec4 blend)
		{
			vec4 mixed = mix(1.0 - 2.0 * (1.0 - base) * (1.0 - blend), 2.0 * base * blend, step(base, vec4(0.5)));

			// mixed = mix(mixed, blend, base.a); // proper alpha mixing?

			return mixed;
		}

        void main()
        {
			vec2 funnyUv = openfl_TextureCoordv;
            vec4 color = flixel_texture2D(bitmap, funnyUv);

			vec2 reallyFunnyUv = vec2(vec2(0.0, 0.0) - gl_FragCoord.xy / openfl_TextureSize.xy);

			vec4 gf = flixel_texture2D(funnyShit, openfl_TextureCoordv.xy + vec2(0.1, 0.2));


			vec4 mixedCol = blendOverlay(color, gf);

            gl_FragColor = mixedCol;
        }

    "#;

#[cfg(test)]
pub const ALL: &[(&str, &str)] = &[("DropShadowShader", DROP_SHADOW_SHADER), ("SserafimShader", SSERAFIM_SHADER), ("OverlayBlend", OVERLAY_BLEND_SHADER)];
