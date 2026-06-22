# AF Fragments and Common Fields with XSD List

### Banking relationship, Product

<table>
<tbody>
<tr>
<th>Fragment Title</th>
<th>Fragment Path</th>
<th>Schema</th>
<th>Complex Type</th>
<th>Auto form conversion support</th>
</tr>
<tr>
<td>UBS Fragment - Banking Relationship 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_BankingRelationship1</td>
<td>BankingRelationship.xsd</td>
<td>BankingRelationshipType</td>
<td> </td>
</tr>
<tr>
<td>UBS Fragment - Custody Account 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_CustodyAccount1</td>
<td>Products.xsd</td>
<td>CustodyAccountType</td>
<td> </td>
</tr>
<tr>
<td>UBS Fragment - Safe Deposit Box 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SafeDepositBox1</td>
<td>Products.xsd</td>
<td>SafeDepositBoxType</td>
<td><br />
</td>
</tr>
<tr>
<td>UBS Fragment - IBAN 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_IBAN1</td>
<td>IBAN.xsd</td>
<td>IBANNumberType</td>
<td><br />
</td>
</tr>
</tbody>
</table>

### Individual, Legal Entity, Address

<table>
<tbody>
<tr>
<th>Fragment Title</th>
<th>Fragment Path</th>
<th>Schema</th>
<th>Complex Type</th>
<th>Remarks</th>
<th>Auto form conversion support</th>
</tr>
<tr>
<td>UBS Fragment - Individual Basic 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_IndividualBasic1</td>
<td>Individual.xsd</td>
<td>IndividualBasicType</td>
<td>Name fields only</td>
<td>(indirectly via other fragments)</td>
</tr>
<tr>
<td>UBS Fragment - Entity Basic 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_EntityBasic1</td>
<td>LegalEntity.xsd</td>
<td>LegalEntityBasicType</td>
<td><br />
</td>
<td>(indirectly via other fragments)</td>
</tr>
<tr>
<td>UBS Fragment - Address Generic 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_AddressGeneric1</td>
<td>Address.xsd</td>
<td>AddressType</td>
<td><br />
</td>
<td></td>
</tr>
<tr>
<td>UBS Fragment - Form of Address</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_FormofAddress</td>
<td>Individual.xsd</td>
<td>IndividualBasicType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>UBS Fragment - DOB and Nationality</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_DOBandNationality</td>
<td>Individual.xsd</td>
<td>IndividualBasicType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>UBS Fragment - Date and Country Incorporation</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_DateandCountryIncorporation</td>
<td>LegalEntity.xsd</td>
<td>LegalEntityBasicType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
</tbody>
</table>

### Partner

<table>
<tbody>
<tr>
<th>Fragment Title</th>
<th>XSD Element name</th>
<th><p>Partner Class</p>
<p>(Individual or Company/Entity)</p></th>
<th>Fragment Path</th>
<th>Schema</th>
<th>Complex Type</th>
<th>Remarks</th>
<th>Auto form conversion support</th>
</tr>
<tr>
<td><strong>Contractual Partner</strong> (ContractualPartnerGenericType)</td>
<td><strong> </strong></td>
<td><br />
</td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
</tr>
<tr>
<td rowspan="3">UBS Fragment - Contractual Partner Generic 1<br />
<br />
<br />
</td>
<td>AccountHolder</td>
<td><strong>AccountHolderPartnerClass</strong></td>
<td rowspan="3">/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_ContractualPartnerGeneric1</td>
<td rowspan="3">ContractualPartnerGeneric.xsd</td>
<td rowspan="3">ContractualPartnerGenericType</td>
<td rowspan="3"><p><strong>Radio button name:</strong> RB_CPType</p>
<p><strong>Contractual Panel naming convention:</strong></p>
<p><strong>Non repeating:</strong> PN_CPG</p>
<p><strong>Repeating:</strong> PN_CPGRP</p>
<p><strong>Signature panel naming covention: </strong></p>
<p><strong>Non repeating:</strong> PN_SGN_CPG</p>
<p><strong>Repeating:</strong> PN_SGN_CPGRP</p></td>
<td><br />
</td>
</tr>
<tr>
<td>Lessee</td>
<td><strong>LesseePartnerClass</strong></td>
<td><br />
</td>
</tr>
<tr>
<td>ContractingPartner</td>
<td><strong>ContractingPartnerClass</strong></td>
<td><br />
</td>
</tr>
<tr>
<td rowspan="3">UBS Fragment - Contractual Partner Generic <strong>Basic</strong> 1<br />
<br />
<br />
</td>
<td>AccountHolder</td>
<td><strong>AccountHolderPartnerClass</strong></td>
<td rowspan="3">/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_ContractualPartnerGenericBasic1</td>
<td rowspan="3">ContractualPartnerGeneric.xsd</td>
<td rowspan="3">ContractualPartnerGenericType</td>
<td rowspan="3"><p><strong>Radio button name:</strong> RB_CPType</p>
<p><strong>Contractual Panel naming convention:</strong></p>
<p><strong>Non repeating:</strong> PN_CPGB</p>
<p><strong>Repeating:</strong> PN_CPGBRP</p>
<p><strong>Signature panel naming covention:</strong></p>
<p><strong>Non repeating:</strong> PN_SGN_CPG</p>
<p><strong>Repeating:</strong> PN_SGN_CPGRP</p></td>
<td><br />
</td>
</tr>
<tr>
<td>Lessee</td>
<td><strong>LesseePartnerClass</strong></td>
<td><br />
</td>
</tr>
<tr>
<td>ContractingPartner</td>
<td><strong>ContractingPartnerClass</strong></td>
<td><br />
</td>
</tr>
<tr>
<td><strong>Beneficial Owner</strong> (BeneficialOwnerGenericType)</td>
<td><strong> </strong></td>
<td><strong>BOPartnerClass</strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
</tr>
<tr>
<td rowspan="2">UBS Fragment - Beneficial Owner Generic 1<br />
<br />
<br />
</td>
<td>BeneficialOwner</td>
<td><br />
</td>
<td rowspan="2">/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_BeneficialOwnerGeneric1</td>
<td rowspan="2">BeneficialOwnerGeneric.xsd</td>
<td rowspan="2">BeneficialOwnerGenericType</td>
<td rowspan="2"><p><strong>Panel naming convention:</strong></p>
<p><strong>Non repeating:</strong> PN_BOG</p>
<p><strong>Repeating:</strong> PN_BOGRP</p>
<p><strong>Radio button name:</strong> RB_BOType</p>
<p><strong>Signature:</strong> PN_SGN_BOGRP</p></td>
<td><br />
</td>
</tr>
<tr>
<td>Trustee</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td rowspan="2">UBS Fragment - Beneficial Owner Generic <strong>Basic</strong> 1<br />
<br />
<br />
</td>
<td>BeneficialOwner</td>
<td><br />
</td>
<td rowspan="2">/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_BeneficialOwnerGenericBasic1</td>
<td rowspan="2">BeneficialOwnerGeneric.xsd</td>
<td rowspan="2">BeneficialOwnerGenericType</td>
<td rowspan="2"><p><strong>Panel naming convention:</strong></p>
<p><strong>Non repeating:</strong> PN_BOG</p>
<p><strong>Repeating:</strong> PN_BOGRP</p>
<p><strong>Radio button name:</strong> RB_BOType</p>
<p><strong>Signature:</strong> PN_SGN_BOGRP</p></td>
<td><br />
</td>
</tr>
<tr>
<td>Trustee</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><strong>Power of Attorney</strong> (PowerOfAttorneyGenericType)</td>
<td><strong> </strong></td>
<td><strong>POAPartnerClass</strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
</tr>
<tr>
<td rowspan="4">UBS Fragment - Power of Attorney Generic 1<br />
<br />
<br />
<br />
<br />
</td>
<td>AuthorizedSigner</td>
<td><br />
</td>
<td rowspan="4">/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_PowerofAttorneyGeneric1</td>
<td rowspan="4">PowerOfAttorneyGeneric.xsd</td>
<td rowspan="4">PowerOfAttorneyGenericType</td>
<td rowspan="4"><p><strong>Radio button name:</strong> RB_PAType</p>
<p><strong>Contractual Panel naming convention:</strong></p>
<p><strong>Non repeating:</strong> PN_PAG</p>
<p><strong>Repeating:</strong> PN_PAGRP</p>
<p><strong>Signature panel naming covention: </strong></p>
<p><strong>Non repeating:</strong> PN_SGN_PAG</p>
<p><strong>Repeating:</strong> PN_SGN_PAGRP</p></td>
<td><br />
</td>
</tr>
<tr>
<td>GeneralPOA</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>LimitedPOA</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>EBankingUser</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td rowspan="4">UBS Fragment - Power of Attorney Generic <strong>Basic</strong> 1<br />
<br />
<br />
 <br />
 </td>
<td>AuthorizedSigner</td>
<td><br />
</td>
<td rowspan="4">/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_PowerofAttorneyGenericBasic1</td>
<td rowspan="4">PowerOfAttorneyGeneric.xsd</td>
<td rowspan="4">PowerOfAttorneyGenericType</td>
<td rowspan="4"><p><strong>Radio button name:</strong> RB_PAType</p>
<p><strong>Contractual Panel naming convention:</strong></p>
<p><strong>Non repeating:</strong> PN_PAGB</p>
<p><strong>Repeating:</strong> PN_PAGBRP</p>
<p><strong>Signature panel naming covention: </strong></p>
<p><strong>Non repeating:</strong> PN_SGN_PAG</p>
<p><strong>Repeating:</strong> PN_SGN_PAGRP</p></td>
<td><br />
</td>
</tr>
<tr>
<td>GeneralPOA</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>LimitedPOA</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>EBankingUser</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><strong>Partner To Partner</strong> (PartnerToPartnerGenericType)</td>
<td><strong> </strong></td>
<td><strong>PToPPartnerClass</strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
<td><strong> </strong></td>
</tr>
<tr>
<td rowspan="5">UBS Fragment - Power of Attorney Generic 1<br />
<br />
<br />
<br />
<br />
<br />
</td>
<td>FIM</td>
<td><br />
</td>
<td rowspan="5">/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_PartnertoPartnerGeneric1</td>
<td rowspan="5">PartnerToPartnerGeneric.xsd</td>
<td rowspan="5">PartnerToPartnerGenericType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>ControllingPerson</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>ConnectedParty</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>AuthRep</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>FIMAuthRep</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td rowspan="5">UBS Fragment - Power of Attorney Generic <strong>Basic</strong> 1<br />
<br />
<br />
<br />
<br />
<br />
</td>
<td>FIM</td>
<td><br />
</td>
<td rowspan="5">/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_PartnertoPartnerGenericBasic1</td>
<td rowspan="5">PartnerToPartnerGeneric.xsd</td>
<td rowspan="5">PartnerToPartnerGenericType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>ControllingPerson</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>ConnectedParty</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>AuthRep</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>FIMAuthRep</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
</tbody>
</table>

### Signature

<table>
<tbody>
<tr>
<th>Signature of</th>
<th>Element name</th>
<th>Fragment Title (if fragment)</th>
<th>Fragment Path (if fragment)</th>
<th>Complex Type</th>
<th>Remarks</th>
<th>Auto form conversion support</th>
</tr>
<tr>
<td><p>Authorized representative</p></td>
<td>AuthRepSignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Legal representative (ACAL)</p></td>
<td>LegalRepSignature</td>
<td> </td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Authorized agent</p></td>
<td>AuthAgentSignature</td>
<td> </td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Account holder</p></td>
<td rowspan="7">AccountHolderSignature<br />
 <br />
<br />
</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Account holder</p></td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Client</p></td>
<td><p>UBS Fragment - Signature Generic 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Pension holder</p></td>
<td><p>UBS Fragment - Signature Generic 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Principal</p></td>
<td><p>UBS Fragment - Signature Generic 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>End Client</p></td>
<td><p>UBS Fragment - Signature Generic 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Recipient of the Risk Information</p></td>
<td><p>UBS Fragment - Signature Generic 1</p>
<p>AAZE</p></td>
<td><br />
</td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Co-owner</p></td>
<td>CoOwnerSignature</td>
<td> </td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Attorney</p></td>
<td>AttorneySignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Client advisor</p></td>
<td>CASignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><strong> </strong></td>
</tr>
<tr>
<td><p>Desk head</p></td>
<td>DeskHeadSignature</td>
<td><p>UBS Fragment - Signature Generic 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Supervisor</p></td>
<td>SupervisorSignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td><p>Financial intermediary</p></td>
<td>FIMSignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><strong> </strong></td>
</tr>
<tr>
<td><p>Sales manager (ABMS-to correct)</p></td>
<td>OPSMSignature &gt; SalesMgrSignature</td>
<td> UBS Fragment - Signature Generic 1</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Relevant person</p></td>
<td>RelevantPersonSignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td><p>Responsible person (ABYT)</p></td>
<td>ResponsiblePersonSignature</td>
<td> UBS Fragment - Signature Generic 1</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Borrower (ABYT, ACAM)</p></td>
<td>BorrowerSignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Beneficial Owner (AAXN)</p></td>
<td>BeneficialOwnerSignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Contracting Partner</p></td>
<td>ContractingPartnerSignature</td>
<td>UBS Fragment - Signature Generic 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>&lt;Hybrid Cases or Unknown type&gt;</p></td>
<td>Signature</td>
<td>AAZE</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Cardholder</p></td>
<td>CardholderSignature</td>
<td>ABME, ABLS</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>UBS Bahrain BRO</p></td>
<td>BahrainBROSignature</td>
<td>BBEE</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>UBS Bahrain CEO / co-CEO</p></td>
<td>BahrainCEOSignature</td>
<td>BBEE</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Company</p></td>
<td>CompanySignature</td>
<td>ABLS</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Pledgor</p></td>
<td>PledgorSignature</td>
<td>AAZB (Formset)</td>
<td><p>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Custody Account Holder</p></td>
<td>CAHSignature1</td>
<td>UBS Fragment - Signature Generic 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SignatureGeneric1</td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
</tbody>
</table>

### AF Component \> XSD Element Type

<table>
<tbody>
<tr>
<th scope="col">AF Component</th>
<th scope="col">XSD Complex/Simple Type Element</th>
<th scope="col">Note</th>
</tr>
<tr>
<td>Email</td>
<td>EmailType</td>
<td>include Address.xsd</td>
</tr>
<tr>
<td>Telephone</td>
<td>PhoneType</td>
<td>include Address.xsd</td>
</tr>
<tr>
<td>Text Box</td>
<td>xs:string</td>
<td><br />
</td>
</tr>
<tr>
<td>Text Box Multiline</td>
<td>xs:string</td>
<td><br />
</td>
</tr>
<tr>
<td>Dropdown List</td>
<td>xs:string</td>
<td><br />
</td>
</tr>
<tr>
<td>Check Box</td>
<td>xs:string</td>
<td rowspan="2">Even the schema data type is string, please use numeric value as key for each option starting from 1.</td>
</tr>
<tr>
<td>Radio Button</td>
<td>xs:string</td>
</tr>
<tr>
<td>Numeric Box</td>
<td>xs:decimal</td>
<td><br />
</td>
</tr>
<tr>
<td>Date Picker</td>
<td>xs:date</td>
<td><br />
</td>
</tr>
</tbody>
</table>

  

<table>
<tbody>
<tr>
<th contenteditable="false"><br />
</th>
<th colspan="7" scope="colgroup"><h3 id="AFFragmentsandCommonFieldswithXSDList-CardBankingFragments">Card Banking Fragments</h3></th>
</tr>
<tr>
<th contenteditable="false" scope="col"><br />
</th>
<th scope="col">Fragment Title</th>
<th scope="col">Element name</th>
<th scope="col">Fragment Path</th>
<th scope="col">Schema</th>
<th scope="col">Complex Type</th>
<th scope="col">Formcodes</th>
<th scope="col">Remarks</th>
</tr>
<tr>
<td contenteditable="false">1</td>
<td>CH Fragment - CC Card Holder 1</td>
<td>CardHolder</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_CardHolder1</td>
<td>CardBanking.xsd</td>
<td>CardHolderType</td>
<td>BAXE, BAXF</td>
<td><br />
</td>
</tr>
<tr>
<td contenteditable="false">2</td>
<td>CH Fragment - CC Additional Services 1</td>
<td>CardAdditionalService</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_AdditionalServices1</p></td>
<td>CardBanking.xsd</td>
<td>CardAdditionalServiceType</td>
<td>BAXE, BAXF</td>
<td>amount differs per type of card/form</td>
</tr>
<tr>
<td contenteditable="false">3</td>
<td>CH Fragment - CC Employment 1</td>
<td>CardEmployment</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_Employment1</p></td>
<td>CardBanking.xsd</td>
<td>CardEmploymentType</td>
<td>BAXE</td>
<td><br />
</td>
</tr>
<tr>
<td contenteditable="false">4</td>
<td>CH Fragment - CC Training 1</td>
<td>CardTraining</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_Training1</p></td>
<td>CardBanking.xsd</td>
<td>CardTrainingType</td>
<td>BAXE</td>
<td>not in BAXF</td>
</tr>
<tr>
<td contenteditable="false">5</td>
<td>CH Fragment - CC Banking Details 1</td>
<td>CardBank</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_BankingDetails</p></td>
<td>CardBanking.xsd</td>
<td>CardBankType</td>
<td>BAXD,BAXE</td>
<td><br />
</td>
</tr>
<tr>
<td contenteditable="false">6</td>
<td>CH Fragment - CC Individual Address 1</td>
<td><p>CardMailingAddress /</p>
<p>CardInvoiceAddress /</p>
<p>CardPartnerAddress</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_IndividualAddress1</p></td>
<td>CardBanking.xsd</td>
<td>CardAddressType</td>
<td>BAXD,BAXE</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td contenteditable="false">7</td>
<td>CH Fragment - CC Legal Representative 1</td>
<td>CardLegalRep</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_LegalRepresentative1</p></td>
<td>CardBanking.xsd</td>
<td>CardLegalRepType</td>
<td>BAXC</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td contenteditable="false">8</td>
<td>CH Fragment - CC Signature Generic 1</td>
<td>CardHolderSignature / CardHolderPartnerSignature / CardLegalRepSignature</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_SignatureGeneric1</p></td>
<td>Signature.xsd</td>
<td>SignatureType</td>
<td>BAXD,BAXE</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td contenteditable="false">9</td>
<td>CH Fragment - CC For internal bank use only 1</td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_ForInternalBankUse1</p></td>
<td>n/a</td>
<td>n/a</td>
<td>BAXF</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td contenteditable="false">10</td>
<td>CH Fragment - CC Annual Credit Interest Text 1</td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_AnnualCreditInterestText1</p></td>
<td>n/a</td>
<td>n/a</td>
<td>BAXD,BAXE</td>
<td><p>Static Text only.</p></td>
</tr>
<tr>
<td contenteditable="false">11</td>
<td>CH Fragment - CC Signature Text <strong>(#)</strong></td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_SignatureText<strong>(#)</strong></p></td>
<td>n/a</td>
<td>n/a</td>
<td>BAXD,BAXE</td>
<td><p>Static Text fragment. Simply change the value of <strong>(#)</strong> to 1,2,3...</p></td>
</tr>
<tr>
<td contenteditable="false">12</td>
<td>CH Fragment - CC Prepaid Signature Text <strong>(#)</strong></td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_PrepaidSignatureText<strong>(#)</strong></p></td>
<td>n/a</td>
<td>n/a</td>
<td>BAXD,BAXE</td>
<td><p>Static Text fragment. Simply change the value of <strong>(#)</strong> to 1,2,3...</p></td>
</tr>
<tr>
<td contenteditable="false">13</td>
<td>CH Fragment - CC Clause Prepaid Signature Text <strong>(#)</strong></td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_ClausePrepaidSignatureText<strong>(#)</strong></p></td>
<td><br />
</td>
<td><br />
</td>
<td>BAXB,BAXC</td>
<td><p>Static Text fragment. Simply change the value of <strong>(#)</strong> to 1,2,3...</p></td>
</tr>
</tbody>
</table>

  

Old Data Model

### Banking relationship, Product

<table>
<colgroup>
<col style="width: 20%" />
<col style="width: 20%" />
<col style="width: 20%" />
<col style="width: 20%" />
<col style="width: 20%" />
</colgroup>
<tbody>
<tr>
<th>Fragment Title</th>
<th>Fragment Path</th>
<th>Schema</th>
<th>Complex Type</th>
<th>Auto form conversion support</th>
</tr>
<tr>
<td>UBS Fragment - Banking Relationship 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_BankingRelationship1</td>
<td>BankingRelationship.xsd</td>
<td>BankingRelationshipType</td>
<td> </td>
</tr>
<tr>
<td>UBS Fragment - Custody Account 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_CustodyAccount1</td>
<td>Products.xsd</td>
<td>CustodyAccountType</td>
<td> </td>
</tr>
<tr>
<td>UBS Fragment - Safe Deposit Box 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_SafeDepositBox1</td>
<td>Products.xsd</td>
<td>SafeDepositBoxType</td>
<td><br />
</td>
</tr>
<tr>
<td>UBS Fragment - IBAN 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_IBAN1</td>
<td>IBAN.xsd</td>
<td>IBANNumberType</td>
<td><br />
</td>
</tr>
</tbody>
</table>

### Individual, Legal Entity, Address

<table>
<tbody>
<tr>
<th>Fragment Title</th>
<th>Fragment Path</th>
<th>Schema</th>
<th>Complex Type</th>
<th>Remarks</th>
<th>Auto form conversion support</th>
</tr>
<tr>
<td>CH Fragment - Individual Basic 1</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_IndividualBasic1</td>
<td>Individual.xsd</td>
<td>IndividualBasicType</td>
<td>Name fields only</td>
<td>(indirectly via other fragments)</td>
</tr>
<tr>
<td>CH Fragment - Individual Basic 2</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_IndividualBasic2</td>
<td>Individual.xsd</td>
<td>IndividualBasicType</td>
<td><p>Name fields + Other common fields for individual case</p>
<p>Please use below script to hide whatever value(s) not required for Form of address field on your form:<br />
<strong>window.forms.ubs.components.hideOptions(["3", "4"], DD_FOA);</strong></p></td>
<td><p>  (indirectly via other fragments)</p></td>
</tr>
<tr>
<td>UBS Fragment - Entity Basic 1</td>
<td>/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_EntityBasic1</td>
<td>LegalEntity.xsd</td>
<td>LegalEntityBasicType</td>
<td><br />
</td>
<td>(indirectly via other fragments)</td>
</tr>
<tr>
<td>CH Fragment - Address 1</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_Address1</td>
<td>Address.xsd</td>
<td>AddressType</td>
<td><br />
</td>
<td></td>
</tr>
<tr>
<td>CH Fragment - Adress Vested Benefits Foundation 1</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AddressVestBenFound1</td>
<td>Address.xsd</td>
<td>Address.xsd</td>
<td><br />
</td>
<td><br />
</td>
</tr>
</tbody>
</table>

### Partner

<table>
<tbody>
<tr>
<th>Fragment Title</th>
<th>Element name</th>
<th><br />
</th>
<th>Fragment Path</th>
<th>Schema</th>
<th>Complex Type</th>
<th>Remarks</th>
<th>Auto form conversion support</th>
</tr>
<tr>
<td>CH Fragment - Account Holder 1</td>
<td>AccountHolder</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AccountHolder1</td>
<td>ContractualPartner.xsd</td>
<td>AccountHolderType</td>
<td><p>For existing hide/show and name reflection script to work as is, please use below field names. </p>
<ul>
<li>RB_ClientType - Individual/Company radio button's for hide/show of names in initialize &amp; save draft case, you still need to add the script in value commit event</li>
<li>PN_AHRP - Repeating panel fragment for signature name reflection</li>
</ul></td>
<td><p> </p></td>
</tr>
<tr>
<td>CH Fragment - Account Holder Depot 1</td>
<td><br />
</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AccountHolder1</td>
<td>ContractualPartner.xsd</td>
<td>AccountHolderType</td>
<td><p>Same as above fragment but with different DE translation for Account holder = Depotinhaber (use case ABSD)</p></td>
<td><p><br />
</p></td>
</tr>
<tr>
<td>CH Fragment - Client 1</td>
<td>AccountHolder</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_Client1</td>
<td>ContractualPartner.xsd</td>
<td>AccountHolderType</td>
<td><p>For existing hide/show and name reflection script to work as is, please use below field names. </p>
<ul>
<li>PN_AHRP - Repeating panel fragment for signature name reflection</li>
</ul></td>
<td><p><br />
</p></td>
</tr>
<tr>
<td>CH Fragment - Account Holder Basic 1</td>
<td>AccountHolderBasic</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AccountHolderBasic1</td>
<td>ContractualPartner.xsd</td>
<td>AccountHolderBasicType</td>
<td><p><br />
</p></td>
<td><p> </p></td>
</tr>
<tr>
<td>CH Fragment - Account Holder Basic 2</td>
<td>AccountHolderBasic</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AccountHolderBasic2</td>
<td>ContractualPartner.xsd</td>
<td>AccountHolderBasicType</td>
<td><p><br />
</p></td>
<td></td>
</tr>
<tr>
<td>CH Fragment - Partner (Individual) 1</td>
<td><br />
</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_PartnerIndividual1</td>
<td>Partner.xsd</td>
<td>PartnerType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td>CH Fragment - Attorney 1</td>
<td><br />
</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_Attorney1</td>
<td>PowerOfAttorney.xsd</td>
<td>GeneralPOAType</td>
<td><p>For existing hide/show and name reflection script to work as is, please use below field names. </p>
<ul>
<li>PN_ATRP - Repeating panel fragment for signature name reflection<br />
(Sample form: AAVV)</li>
</ul>
<p><br />
</p>
<p>Unchecked "Required field" on properties for Nationality to fix issue while activating Print Incomplete</p></td>
<td></td>
</tr>
<tr>
<td>CH Fragment - Authorized Representative 1</td>
<td><br />
</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AuthRep1</td>
<td>PartnerToPartner.xsd</td>
<td>AuthRepType</td>
<td><p><br />
</p></td>
<td><p><br />
</p></td>
</tr>
<tr>
<td>CH Fragment - FIM 1</td>
<td><br />
</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_FIM1</td>
<td>PartnerToPartner.xsd</td>
<td>FIMType</td>
<td><p><br />
</p></td>
<td><p><br />
</p></td>
</tr>
<tr>
<td>CH Fragment - FIM 2</td>
<td>FIM</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_FIM2</td>
<td>PartnerToPartner.xsd</td>
<td>FIMType</td>
<td><p><br />
</p></td>
<td><p><br />
</p></td>
</tr>
<tr>
<td>CH Fragment - Custody Account Holder Basic 2</td>
<td>CustodyAccountHolderBasic2</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_CustodyAccountHolderBasic2</td>
<td>ContractualPartner.xsd</td>
<td>AccountHolderType</td>
<td><p>Fragment is designed with latest IC-Core-script (Feb 2026)</p></td>
<td><p><br />
</p></td>
</tr>
<tr>
<td>CH Fragment - Account Holder Vested Benefits Foundation 1</td>
<td>AccountHolderVestBenFound1</td>
<td><br />
</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AccountHolderVestBenFound1</td>
<td>ContractualPartner.xsd</td>
<td>AccountHolderType</td>
<td><p><br />
</p></td>
<td><p><br />
</p></td>
</tr>
</tbody>
</table>

### Signature

<table>
<tbody>
<tr>
<th>Signature of</th>
<th>Element name</th>
<th>Fragment Title (if fragment)</th>
<th>Fragment Path (if fragment)</th>
<th>Complex Type</th>
<th>Remarks</th>
<th>Auto form conversion support</th>
</tr>
<tr>
<td><p>Authorized representative</p></td>
<td>AuthRepSignature</td>
<td>CH Fragment - Signature (Authorized Representative) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_ARSignature1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Legal representative (ACAL)</p></td>
<td>LegalRepSignature</td>
<td> </td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Authorized agent</p></td>
<td>AuthAgentSignature</td>
<td> </td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Account holder</p></td>
<td rowspan="7">AccountHolderSignature<br />
 <br />
<br />
</td>
<td>CH Fragment - Signature (Account Holder) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AHSignature1</p></td>
<td>SignatureType</td>
<td>Automatic name reflection script is added and should work as is for repeating instance case as long as the Account Holder Repeating Panel Fragment name is PN_AHRP. For single instance case, a hidden field needs to be added as a temporary workaround. </td>
<td><br />
</td>
</tr>
<tr>
<td><p>Account holder</p></td>
<td>CH Fragment - Signature (Account Holder Depot) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AHSignatureDepot1</p></td>
<td>SignatureType</td>
<td>Same as above fragment but with different DE translation for Account holder = Depotinhaber (use case ABSD)</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Client</p></td>
<td><p>CH Fragment - Signature (Client) 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_ClientSignature1</p></td>
<td>SignatureType</td>
<td>Automatic name reflection script is added and should work as is for repeating instance case as long as the Client Repeating Panel Fragment name is PN_AHRP. For single instance case, a hidden field needs to be added as a temporary workaround. </td>
<td><br />
</td>
</tr>
<tr>
<td><p>Pension holder</p></td>
<td><p>CH Fragment - Signature (Pension Holder) 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_PensionHSignature1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Principal</p></td>
<td><p>CH Fragment - Signature (Principal) 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_PrincipalSignature1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>End Client</p></td>
<td><p>CH Fragment - Signature (End Client) 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_EndClientSignature1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Recipient of the Risk Information</p></td>
<td><p>AAZE</p></td>
<td><br />
</td>
<td>SignatureType</td>
<td>Automatic name reflection script is added and should work as is for repeating instance case as long as the Account Holder Repeating Panel Fragment name is PN_AHRP. For single instance case, a hidden field needs to be added as a temporary workaround. </td>
<td><br />
</td>
</tr>
<tr>
<td><p>Co-owner</p></td>
<td>CoOwnerSignature</td>
<td> </td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Attorney</p></td>
<td>AttorneySignature</td>
<td>CH Fragment - Signature (Attorney) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_AttySignature1</p></td>
<td>SignatureType</td>
<td>Automatic name reflection script is added and should work as is for repeating instance case as long as the Panel Fragment name is PN_AttyRP. For single instance case, a hidden field needs to be added as a temporary workaround. </td>
<td><br />
</td>
</tr>
<tr>
<td><p>Client advisor</p></td>
<td>CASignature</td>
<td>CH Fragment - Signature (Client Advisor) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_CASignature1</p></td>
<td>SignatureType</td>
<td><strong>In the fragment panel's DoR config, use 6 number of columns for the DoR layout.</strong></td>
<td><strong> </strong></td>
</tr>
<tr>
<td><p>Desk head</p></td>
<td>DeskHeadSignature</td>
<td><p>CH Fragment - Signature (Desk Head) 1</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_DeskHeadSignature1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Supervisor</p></td>
<td>SupervisorSignature</td>
<td>CH Fragment - Signature (Supervisor) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_SupervisorSignature1</p></td>
<td>SignatureType</td>
<td><p>To confirm where to bind OURef field and if can be considered common field in Signature</p>
<p><strong>In the fragment panel's DoR config, use 6 number of columns for the DoR layout.</strong></p></td>
<td><p><br />
</p></td>
</tr>
<tr>
<td><p>Financial intermediary</p></td>
<td>FIMSignature</td>
<td>CH Fragment - Signature (FIM) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_FIMSignature1</p></td>
<td>SignatureType</td>
<td><strong>In the fragment panel's DoR config, use 6 number of columns for the DoR layout.</strong></td>
<td><strong> </strong></td>
</tr>
<tr>
<td><p>Sales manager (ABMS-to correct)</p></td>
<td>OPSMSignature &gt; SalesMgrSignature</td>
<td> </td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Relevant person</p></td>
<td>RelevantPersonSignature</td>
<td>CH Fragment - Signature (Relevant Person) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_RelevantPSignature1</p></td>
<td>SignatureType</td>
<td><p>For automatic name reflection, use below paths for name fields</p>
<ul>
<li>PN_RelevantP.TXT_LastName_RelP</li>
<li>PN_RelevantP.TXT_FirstName_RelP</li>
</ul></td>
<td><p><br />
</p></td>
</tr>
<tr>
<td><p>Responsible person (ABYT)</p></td>
<td>ResponsiblePersonSignature</td>
<td> </td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Borrower (ABYT, ACAM)</p></td>
<td>BorrowerSignature</td>
<td> CH Fragment - Signature (Borrower) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_BRSignature1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Beneficial Owner (AAXN)</p></td>
<td>BeneficialOwnerSignature</td>
<td>CH Fragment - Signature (Beneficial Owner) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_BOSignature1</p></td>
<td>SignatureType</td>
<td>Automatic name reflection script is added and should work as is for repeating instance case as long as the Account Holder Repeating Panel Fragment name is PN_BORP.</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Contracting Partner</p></td>
<td>ContractingPartnerSignature</td>
<td> CH Fragment - Signature (Contracting Partner) 1</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_ContractingPartnerSignature1</p></td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>&lt;Hybrid Cases or Unknown type&gt;</p></td>
<td>Signature</td>
<td>AAZE</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Cardholder</p></td>
<td>CardholderSignature</td>
<td>ABME, ABLS</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>UBS Bahrain BRO</p></td>
<td>BahrainBROSignature</td>
<td>BBEE</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>UBS Bahrain CEO / co-CEO</p></td>
<td>BahrainCEOSignature</td>
<td>BBEE</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Company</p></td>
<td>CompanySignature</td>
<td>ABLS</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Pledgor</p></td>
<td>PledgorSignature</td>
<td>AAZB (Formset)</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_Signature-Pledgor</td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Custody Account Holder</p></td>
<td>CAHSignature1</td>
<td>CH Fragment - Signature (CustodyAccount Holder) 1</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_CAHSignature1</td>
<td>SignatureType</td>
<td>Fragment is designed with latest IC-Core-script (Feb 2026)</td>
<td><br />
</td>
</tr>
<tr>
<td><p>Authorized Representative Fisca</p></td>
<td>ARSignatureFisca1</td>
<td>CH Fragment - Signature (Authorized RepresentativeFisca) 1</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_ARSignatureFisca1</td>
<td>SignatureType</td>
<td><br />
</td>
<td><br />
</td>
</tr>
</tbody>
</table>

### AF Component \> XSD Element Type

<table>
<tbody>
<tr>
<th>AF Component</th>
<th>XSD Complex/Simple Type Element</th>
<th>Note</th>
</tr>
<tr>
<td>Email</td>
<td>EmailType</td>
<td>include Address.xsd</td>
</tr>
<tr>
<td>Telephone</td>
<td>PhoneType</td>
<td>include Address.xsd</td>
</tr>
<tr>
<td>Text Box</td>
<td>xs:string</td>
<td><br />
</td>
</tr>
<tr>
<td>Text Box Multiline</td>
<td>xs:string</td>
<td><br />
</td>
</tr>
<tr>
<td>Dropdown List</td>
<td>xs:string</td>
<td><br />
</td>
</tr>
<tr>
<td>Check Box</td>
<td>xs:string</td>
<td rowspan="2">Even the schema data type is string, please use numeric value as key for each option starting from 1.</td>
</tr>
<tr>
<td>Radio Button</td>
<td>xs:string</td>
</tr>
<tr>
<td>Numeric Box</td>
<td>xs:decimal</td>
<td><br />
</td>
</tr>
<tr>
<td>Date Picker</td>
<td>xs:date</td>
<td><br />
</td>
</tr>
</tbody>
</table>

  

<table>
<tbody>
<tr>
<th contenteditable="false"><br />
</th>
<th colspan="7"><h3 id="AFFragmentsandCommonFieldswithXSDList-CardBankingFragments.1">Card Banking Fragments</h3></th>
</tr>
<tr>
<th contenteditable="false"><br />
</th>
<th>Fragment Title</th>
<th>Element name</th>
<th>Fragment Path</th>
<th>Schema</th>
<th>Complex Type</th>
<th>Formcodes</th>
<th>Remarks</th>
</tr>
<tr>
<td contenteditable="false">1</td>
<td>CH Fragment - CC Card Holder 1</td>
<td>CardHolder</td>
<td>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_CardHolder1</td>
<td>CardBanking.xsd</td>
<td>CardHolderType</td>
<td>BAXE, BAXF</td>
<td><br />
</td>
</tr>
<tr>
<td contenteditable="false">2</td>
<td>CH Fragment - CC Additional Services 1</td>
<td>CardAdditionalService</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_AdditionalServices1</p></td>
<td>CardBanking.xsd</td>
<td>CardAdditionalServiceType</td>
<td>BAXE, BAXF</td>
<td>amount differs per type of card/form</td>
</tr>
<tr>
<td contenteditable="false">3</td>
<td>CH Fragment - CC Employment 1</td>
<td>CardEmployment</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_Employment1</p></td>
<td>CardBanking.xsd</td>
<td>CardEmploymentType</td>
<td>BAXE</td>
<td><br />
</td>
</tr>
<tr>
<td contenteditable="false">4</td>
<td>CH Fragment - CC Training 1</td>
<td>CardTraining</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_Training1</p></td>
<td>CardBanking.xsd</td>
<td>CardTrainingType</td>
<td>BAXE</td>
<td>not in BAXF</td>
</tr>
<tr>
<td contenteditable="false">5</td>
<td>CH Fragment - CC Banking Details 1</td>
<td>CardBank</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_BankingDetails</p></td>
<td>CardBanking.xsd</td>
<td>CardBankType</td>
<td>BAXD,BAXE</td>
<td><br />
</td>
</tr>
<tr>
<td contenteditable="false">6</td>
<td>CH Fragment - CC Individual Address 1</td>
<td><p>CardMailingAddress /</p>
<p>CardInvoiceAddress /</p>
<p>CardPartnerAddress</p></td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_IndividualAddress1</p></td>
<td>CardBanking.xsd</td>
<td>CardAddressType</td>
<td>BAXD,BAXE</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td contenteditable="false">7</td>
<td>CH Fragment - CC Legal Representative 1</td>
<td>CardLegalRep</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_LegalRepresentative1</p></td>
<td>CardBanking.xsd</td>
<td>CardLegalRepType</td>
<td>BAXC</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td contenteditable="false">8</td>
<td>CH Fragment - CC Signature Generic 1</td>
<td>CardHolderSignature / CardHolderPartnerSignature / CardLegalRepSignature</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_SignatureGeneric1</p></td>
<td>Signature.xsd</td>
<td>SignatureType</td>
<td>BAXD,BAXE</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td contenteditable="false">9</td>
<td>CH Fragment - CC For internal bank use only 1</td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_ForInternalBankUse1</p></td>
<td>n/a</td>
<td>n/a</td>
<td>BAXF</td>
<td><p><br />
</p></td>
</tr>
<tr>
<td contenteditable="false">10</td>
<td>CH Fragment - CC Annual Credit Interest Text 1</td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_AnnualCreditInterestText1</p></td>
<td>n/a</td>
<td>n/a</td>
<td>BAXD,BAXE</td>
<td><p>Static Text only.</p></td>
</tr>
<tr>
<td contenteditable="false">11</td>
<td>CH Fragment - CC Signature Text <strong>(#)</strong></td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_SignatureText<strong>(#)</strong></p></td>
<td>n/a</td>
<td>n/a</td>
<td>BAXD,BAXE</td>
<td><p>Static Text fragment. Simply change the value of <strong>(#)</strong> to 1,2,3...</p></td>
</tr>
<tr>
<td contenteditable="false">12</td>
<td>CH Fragment - CC Prepaid Signature Text <strong>(#)</strong></td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_PrepaidSignatureText<strong>(#)</strong></p></td>
<td>n/a</td>
<td>n/a</td>
<td>BAXD,BAXE</td>
<td><p>Static Text fragment. Simply change the value of <strong>(#)</strong> to 1,2,3...</p></td>
</tr>
<tr>
<td contenteditable="false">13</td>
<td>CH Fragment - CC Clause Prepaid Signature Text <strong>(#)</strong></td>
<td>n/a</td>
<td><p>/content/dam/formsanddocuments/afforms_ch_fragmentlib/affrg_cc_ClausePrepaidSignatureText<strong>(#)</strong></p></td>
<td><br />
</td>
<td><br />
</td>
<td>BAXB,BAXC</td>
<td><p>Static Text fragment. Simply change the value of <strong>(#)</strong> to 1,2,3...</p></td>
</tr>
</tbody>
</table>

  
